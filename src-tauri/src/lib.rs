use base64::{engine::general_purpose, Engine as _};
use id3::TagLike;
use lofty::prelude::{TagExt, TaggedFileExt};
use lofty::probe::Probe;
use serde::{Deserialize, Serialize};
use soulseek_rs::types::DownloadStatus as SoulseekDownloadStatus;
use soulseek_rs::{Client, ClientSettings, SearchResult};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU16, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use tauri::window::{ProgressBarState, ProgressBarStatus};
use tauri::{AppHandle, Emitter, Manager};

#[cfg(target_os = "windows")]
use window_vibrancy::{apply_acrylic, clear_acrylic};

#[cfg(target_os = "macos")]
use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial};

#[tauri::command]
fn set_window_material(window: tauri::WebviewWindow, material: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let _ = clear_acrylic(&window);

        if material == "acrylic" {
            apply_acrylic(&window, Some((18, 18, 18, 115))).map_err(|e| e.to_string())?;
        }
    }

    #[cfg(target_os = "macos")]
    {
        if material != "none" {
            apply_vibrancy(&window, NSVisualEffectMaterial::HudWindow, None, Some(16.0))
                .map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .manage(SoulseekState::default())
        .manage(DownloadState::default())
        .setup(|app| {
            let window = app.get_webview_window("main").unwrap();

            #[cfg(target_os = "windows")]
            {
                let _ = apply_acrylic(&window, Some((18, 18, 18, 115)));
            }

            #[cfg(target_os = "macos")]
            {
                let _ =
                    apply_vibrancy(&window, NSVisualEffectMaterial::HudWindow, None, Some(16.0));
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            set_window_material,
            soulseek_search,
            queue_downloads,
            pause_download,
            resume_download,
            delete_download,
            reorder_downloads,
            scan_library,
            read_audio_data_url,
            delete_library_album,
            load_album_cover,
            set_taskbar_progress,
            edit_library_album
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri app");
}

/* ----------------------------- APP STATE ----------------------------- */

#[derive(Clone)]
struct SoulseekState {
    inner: Arc<SoulseekInner>,
}

struct SoulseekInner {
    client: Mutex<Option<Arc<Client>>>,
    username: Mutex<Option<String>>,
}

impl Default for SoulseekState {
    fn default() -> Self {
        Self {
            inner: Arc::new(SoulseekInner {
                client: Mutex::new(None),
                username: Mutex::new(None),
            }),
        }
    }
}

#[derive(Clone)]
struct DownloadState {
    inner: Arc<DownloadInner>,
}

struct DownloadInner {
    jobs: Mutex<HashMap<String, DownloadJob>>,
    worker_running: AtomicBool,
}

impl Default for DownloadState {
    fn default() -> Self {
        Self {
            inner: Arc::new(DownloadInner {
                jobs: Mutex::new(HashMap::new()),
                worker_running: AtomicBool::new(false),
            }),
        }
    }
}

/* ----------------------------- SEARCH TYPES ----------------------------- */

#[derive(Debug, Deserialize)]
struct TaskbarProgressRequest {
    progress: Option<u64>,
    status: String,
}

#[tauri::command]
fn set_taskbar_progress(
    window: tauri::WebviewWindow,
    req: TaskbarProgressRequest,
) -> Result<(), String> {
    let status = match req.status.as_str() {
        "normal" => Some(ProgressBarStatus::Normal),
        "paused" => Some(ProgressBarStatus::Paused),
        "error" => Some(ProgressBarStatus::Error),
        "indeterminate" => Some(ProgressBarStatus::Indeterminate),
        _ => Some(ProgressBarStatus::None),
    };

    window
        .set_progress_bar(ProgressBarState {
            status,
            progress: req.progress,
        })
        .map_err(|e| e.to_string())
}

#[derive(Debug, Deserialize)]
struct SoulseekSearchRequest {
    username: String,
    password: String,
    query: String,
}

#[derive(Debug, Serialize, Clone)]
struct UiSearchResult {
    username: String,
    folder: String,
    name: String,
    size: u64,
    attributes: String,
    duration_seconds: Option<u32>,
}

#[derive(Debug, Serialize, Clone)]
struct SearchResultsEvent {
    query: String,
    results: Vec<UiSearchResult>,
    done: bool,
    error: Option<String>,
}

/* ----------------------------- DOWNLOAD TYPES ----------------------------- */

#[derive(Debug, Deserialize, Clone)]
struct QueueDownloadItem {
    id: String,
    package_id: String,
    package_name: String,
    username: String,
    folder: String,
    name: String,
    path: String,
    size: u64,
}

#[derive(Debug, Deserialize)]
struct QueueDownloadsRequest {
    username: String,
    password: String,
    download_folder: String,
    max_concurrent: Option<usize>,
    items: Vec<QueueDownloadItem>,
}

#[derive(Debug, Deserialize)]
struct DownloadIdRequest {
    id: String,
}

#[derive(Debug, Deserialize)]
struct ReorderDownloadsRequest {
    ids: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
struct DownloadEvent {
    id: String,
    username: String,
    folder: String,
    name: String,
    size: u64,
    status: String,
    progress: f32,
    speed_bytes_per_sec: f32,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct DownloadJob {
    item: QueueDownloadItem,
    status: DownloadStatus,
    progress: f32,
    priority: usize,
    error: Option<String>,
    pending_dir: PathBuf,
    final_dir: PathBuf,
    downloaded_bytes: u64,
    last_speed_bytes: u64,
    last_speed_at: Instant,
    speed_bytes_per_sec: f32,
}

#[derive(Debug, Clone, PartialEq)]
enum DownloadStatus {
    Queued,
    Downloading,
    Paused,
    Done,
    Failed,
    Deleted,
}

impl DownloadStatus {
    fn as_str(&self) -> &'static str {
        match self {
            DownloadStatus::Queued => "queued",
            DownloadStatus::Downloading => "downloading",
            DownloadStatus::Paused => "paused",
            DownloadStatus::Done => "done",
            DownloadStatus::Failed => "failed",
            DownloadStatus::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Deserialize)]
struct ScanLibraryRequest {
    download_folder: String,
}

#[derive(Debug, Serialize, Clone)]
struct LibraryTrack {
    id: String,
    name: String,
    path: String,
    folder: String,
    album_name: String,
    size: u64,
    extension: String,
    cover_data_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReadAudioFileRequest {
    path: String,
}

/* ----------------------------- SHARED CLIENT ----------------------------- */

static NEXT_LISTEN_PORT: AtomicU16 = AtomicU16::new(50500);

fn clear_stored_client(state: &SoulseekInner) -> Result<(), String> {
    let mut stored_client = state.client.lock().map_err(|e| e.to_string())?;
    let mut stored_username = state.username.lock().map_err(|e| e.to_string())?;

    *stored_client = None;
    *stored_username = None;

    Ok(())
}

fn get_or_connect_client(
    state: &SoulseekInner,
    username: String,
    password: String,
) -> Result<Arc<Client>, String> {
    let mut stored_client = state.client.lock().map_err(|e| e.to_string())?;
    let mut stored_username = state.username.lock().map_err(|e| e.to_string())?;

    let reuse_existing =
        stored_client.is_some() && stored_username.as_deref() == Some(username.as_str());

    if reuse_existing {
        return Ok(stored_client.as_ref().unwrap().clone());
    }

    println!("connecting soulseek client...");

    let listen_port = NEXT_LISTEN_PORT.fetch_add(1, Ordering::SeqCst);

    if listen_port > 50950 {
        NEXT_LISTEN_PORT.store(50500, Ordering::SeqCst);
    }

    let settings = ClientSettings {
        enable_listen: true,
        listen_port,
        ..ClientSettings::new(username.clone(), password)
    };

    let mut raw_client = Client::with_settings(settings);

    raw_client.connect();

    let logged_in = raw_client
        .login()
        .map_err(|e| format!("login failed: {e}"))?;

    if !logged_in {
        return Err("login rejected".to_string());
    }

    println!("soulseek connected on listen port {listen_port}");

    let client = Arc::new(raw_client);

    *stored_client = Some(client.clone());
    *stored_username = Some(username);

    Ok(client)
}

/* ----------------------------- SEARCH COMMAND ----------------------------- */

fn run_search_once(app: &AppHandle, client: Arc<Client>, query: &str, timeout: Duration) -> usize {
    println!("starting soulseek search worker: {query}");

    let cancel = Arc::new(AtomicBool::new(false));

    let worker_client = Arc::clone(&client);
    let worker_cancel = Arc::clone(&cancel);
    let worker_query = query.to_string();

    let worker = std::thread::spawn(move || {
        let _ = worker_client.search_with_cancel(&worker_query, timeout, Some(worker_cancel));
    });

    let start = Instant::now();
    let mut last_count = 0usize;
    let mut last_change_at = Instant::now();

    loop {
        std::thread::sleep(Duration::from_millis(150));

        let raw_results = client.try_get_search_results(query).unwrap_or_default();

        let mut flattened = flatten_search_results(raw_results);
        flattened.truncate(300);

        if flattened.len() != last_count {
            last_count = flattened.len();
            last_change_at = Instant::now();

            println!("streaming {} results for {}", flattened.len(), query);

            let _ = app.emit(
                "soulseek-search-results",
                SearchResultsEvent {
                    query: query.to_string(),
                    results: flattened,
                    done: false,
                    error: None,
                },
            );
        }

        if last_count >= 25 && last_change_at.elapsed() >= Duration::from_millis(1200) {
            cancel.store(true, Ordering::Relaxed);
            break;
        }

        if start.elapsed() >= timeout {
            break;
        }
    }

    cancel.store(true, Ordering::Relaxed);
    let _ = worker.join();

    last_count
}

#[tauri::command]
async fn soulseek_search(
    app: AppHandle,
    soulseek_state: tauri::State<'_, SoulseekState>,
    download_state: tauri::State<'_, DownloadState>,
    req: SoulseekSearchRequest,
) -> Result<(), String> {
    let soulseek_inner = soulseek_state.inner.clone();
    let download_inner = download_state.inner.clone();

    let soulseek_inner = soulseek_state.inner.clone();
    let download_inner = download_state.inner.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let query = req.query;
        let timeout = Duration::from_secs(15);

        println!("search requested: {query}");

        let active_downloads = has_active_downloads(&download_inner);

        if active_downloads {
            println!("active downloads detected; reusing existing soulseek client");
        } else {
            println!("no active downloads; reusing existing soulseek client");
        }

        let client = match get_or_connect_client(
            &soulseek_inner,
            req.username.clone(),
            req.password.clone(),
        ) {
            Ok(client) => client,
            Err(err) => {
                let _ = app.emit(
                    "soulseek-search-results",
                    SearchResultsEvent {
                        query,
                        results: Vec::new(),
                        done: true,
                        error: Some(err),
                    },
                );
                return;
            }
        };

        let mut last_count = run_search_once(&app, client, &query, timeout);

        if last_count == 0 && !active_downloads {
            println!("search returned 0 results; reconnecting and retrying once: {query}");

            let _ = clear_stored_client(&soulseek_inner);

            match get_or_connect_client(&soulseek_inner, req.username.clone(), req.password.clone())
            {
                Ok(retry_client) => {
                    last_count = run_search_once(&app, retry_client, &query, timeout);
                }
                Err(err) => {
                    let _ = app.emit(
                        "soulseek-search-results",
                        SearchResultsEvent {
                            query,
                            results: Vec::new(),
                            done: true,
                            error: Some(err),
                        },
                    );
                    return;
                }
            }
        }

        println!("search finished: {query}, results streamed: {last_count}");

        let _ = app.emit(
            "soulseek-search-results",
            SearchResultsEvent {
                query,
                results: Vec::new(),
                done: true,
                error: None,
            },
        );
    });

    Ok(())
}

/* ----------------------------- DOWNLOAD COMMANDS ----------------------------- */

#[tauri::command]
async fn queue_downloads(
    app: AppHandle,
    soulseek_state: tauri::State<'_, SoulseekState>,
    download_state: tauri::State<'_, DownloadState>,
    req: QueueDownloadsRequest,
) -> Result<(), String> {
    if req.download_folder.trim().is_empty() {
        return Err("Choose a download folder in Settings first.".to_string());
    }

    let client = get_or_connect_client(&soulseek_state.inner, req.username, req.password)?;

    let max_concurrent = req.max_concurrent.unwrap_or(2).clamp(1, 6);
    let inner = download_state.inner.clone();

    {
        let mut jobs = inner.jobs.lock().map_err(|e| e.to_string())?;
        let start_priority = jobs.len();

        for (offset, item) in req.items.into_iter().enumerate() {
            if jobs.contains_key(&item.id) {
                continue;
            }

            let package_name = sanitize_folder_name(&item.package_name);
            let pending_dir = PathBuf::from(&req.download_folder)
                .join("_pending")
                .join(&package_name);
            let final_dir = PathBuf::from(&req.download_folder).join(&package_name);

            fs::create_dir_all(&pending_dir)
                .map_err(|e| format!("failed to create pending folder: {e}"))?;

            let now = Instant::now();

            let job = DownloadJob {
                item,
                status: DownloadStatus::Queued,
                progress: 0.0,
                priority: start_priority + offset,
                error: None,
                pending_dir,
                final_dir,
                downloaded_bytes: 0,
                last_speed_bytes: 0,
                last_speed_at: now,
                speed_bytes_per_sec: 0.0,
            };

            emit_download_event(&app, &job);
            jobs.insert(job.item.id.clone(), job);
        }
    }

    ensure_download_worker(app, client, inner, max_concurrent);

    Ok(())
}

#[tauri::command]
fn pause_download(
    app: AppHandle,
    soulseek_state: tauri::State<'_, SoulseekState>,
    download_state: tauri::State<'_, DownloadState>,
    req: DownloadIdRequest,
) -> Result<(), String> {
    let client = {
        let stored_client = soulseek_state
            .inner
            .client
            .lock()
            .map_err(|e| e.to_string())?;

        stored_client.clone()
    };

    let mut jobs = download_state
        .inner
        .jobs
        .lock()
        .map_err(|e| e.to_string())?;

    let Some(job) = jobs.get_mut(&req.id) else {
        return Ok(());
    };

    if matches!(
        job.status,
        DownloadStatus::Done | DownloadStatus::Failed | DownloadStatus::Deleted
    ) {
        return Ok(());
    }

    if let Some(client) = client {
        let _ = client.pause_download(&job.item.username, &job.item.path);
    }

    job.status = DownloadStatus::Paused;
    emit_download_event(&app, job);

    Ok(())
}

#[tauri::command]
fn resume_download(
    app: AppHandle,
    soulseek_state: tauri::State<'_, SoulseekState>,
    download_state: tauri::State<'_, DownloadState>,
    req: DownloadIdRequest,
) -> Result<(), String> {
    let client = {
        let stored_client = soulseek_state
            .inner
            .client
            .lock()
            .map_err(|e| e.to_string())?;

        stored_client.clone()
    };

    let mut jobs = download_state
        .inner
        .jobs
        .lock()
        .map_err(|e| e.to_string())?;

    let Some(job) = jobs.get_mut(&req.id) else {
        return Ok(());
    };

    if job.status == DownloadStatus::Paused {
        if let Some(client) = client {
            let _ = client.resume_download(&job.item.username, &job.item.path);
        }

        job.status = DownloadStatus::Downloading;
        emit_download_event(&app, job);
    }

    Ok(())
}

#[tauri::command]
fn delete_download(
    app: AppHandle,
    soulseek_state: tauri::State<'_, SoulseekState>,
    download_state: tauri::State<'_, DownloadState>,
    req: DownloadIdRequest,
) -> Result<(), String> {
    let client = {
        let stored_client = soulseek_state
            .inner
            .client
            .lock()
            .map_err(|e| e.to_string())?;

        stored_client.clone()
    };

    let job = {
        let mut jobs = download_state
            .inner
            .jobs
            .lock()
            .map_err(|e| e.to_string())?;

        let Some(job) = jobs.get_mut(&req.id) else {
            return Ok(());
        };

        job.status = DownloadStatus::Deleted;
        job.speed_bytes_per_sec = 0.0;
        job.error = None;

        let cloned = job.clone();
        emit_download_event(&app, job);

        cloned
    };

    if let Some(client) = client {
        let username = job.item.username.clone();
        let path = job.item.path.clone();

        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = client.remove_queued_download(&username, &path);
        }));

        if result.is_err() {
            println!(
                "remove_queued_download panicked; ignored:\n  user: {}\n  path: {}",
                username, path
            );
        }
    }

    {
        let mut jobs = download_state
            .inner
            .jobs
            .lock()
            .map_err(|e| e.to_string())?;

        jobs.remove(&req.id);
    }

    Ok(())
}

#[tauri::command]
fn reorder_downloads(
    download_state: tauri::State<'_, DownloadState>,
    req: ReorderDownloadsRequest,
) -> Result<(), String> {
    let mut jobs = download_state
        .inner
        .jobs
        .lock()
        .map_err(|e| e.to_string())?;

    for (priority, id) in req.ids.iter().enumerate() {
        if let Some(job) = jobs.get_mut(id) {
            job.priority = priority;
        }
    }

    Ok(())
}

/* ----------------------------- LIBRARY COMMANDS ----------------------------- */

#[tauri::command]
fn scan_library(req: ScanLibraryRequest) -> Result<Vec<LibraryTrack>, String> {
    let root = PathBuf::from(req.download_folder);

    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut tracks = Vec::new();

    scan_library_dir(&root, &root, &mut tracks)?;

    tracks.sort_by(|a, b| a.album_name.cmp(&b.album_name).then(a.name.cmp(&b.name)));

    Ok(tracks)
}

fn scan_library_dir(root: &Path, dir: &Path, tracks: &mut Vec<LibraryTrack>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| e.to_string())?;

    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();

        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();

        if file_name == "_pending" || file_name == "_artwork" || file_name == "_transcoded" {
            continue;
        }

        if path.is_dir() {
            scan_library_dir(root, &path, tracks)?;
            continue;
        }

        if !is_audio_file(&path) {
            continue;
        }

        let metadata = fs::metadata(&path).map_err(|e| e.to_string())?;

        let folder = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        let album_name = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "Unknown Album".to_string());

        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "Unknown Track".to_string());

        let extension = path
            .extension()
            .map(|ext| ext.to_string_lossy().to_lowercase())
            .unwrap_or_default();

        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();

        tracks.push(LibraryTrack {
            id: relative,
            name,
            path: path.to_string_lossy().to_string(),
            folder,
            album_name,
            size: metadata.len(),
            extension,
            cover_data_url: None,
        });
    }

    Ok(())
}

fn is_audio_file(path: &Path) -> bool {
    let Some(ext) = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
    else {
        return false;
    };

    matches!(
        ext.as_str(),
        "mp3" | "flac" | "m4a" | "aac" | "ogg" | "opus" | "wav" | "aiff" | "alac"
    )
}

/* ----------------------------- DOWNLOAD WORKER ----------------------------- */

fn has_active_downloads(inner: &DownloadInner) -> bool {
    let Ok(jobs) = inner.jobs.lock() else {
        return true;
    };

    jobs.values().any(|job| {
        matches!(
            job.status,
            DownloadStatus::Queued | DownloadStatus::Downloading | DownloadStatus::Paused
        )
    })
}

fn ensure_download_worker(
    app: AppHandle,
    client: Arc<Client>,
    inner: Arc<DownloadInner>,
    max_concurrent: usize,
) {
    if inner.worker_running.swap(true, Ordering::SeqCst) {
        return;
    }

    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(250));

        let mut to_start: Vec<DownloadJob> = Vec::new();

        {
            let mut jobs = match inner.jobs.lock() {
                Ok(jobs) => jobs,
                Err(_) => break,
            };

            let active_count = jobs
                .values()
                .filter(|job| job.status == DownloadStatus::Downloading)
                .count();

            let available_slots = max_concurrent.saturating_sub(active_count);

            if available_slots > 0 {
                let mut queued: Vec<_> = jobs
                    .values()
                    .filter(|job| job.status == DownloadStatus::Queued)
                    .cloned()
                    .collect();

                queued.sort_by_key(|job| job.priority);

                for job in queued.into_iter().take(available_slots) {
                    if let Some(stored) = jobs.get_mut(&job.item.id) {
                        stored.status = DownloadStatus::Downloading;
                        stored.progress = 0.0;
                        stored.error = None;

                        emit_download_event(&app, stored);
                        to_start.push(stored.clone());
                    }
                }
            }

            let still_has_work = jobs.values().any(|job| {
                matches!(
                    job.status,
                    DownloadStatus::Queued | DownloadStatus::Downloading | DownloadStatus::Paused
                )
            });

            if !still_has_work && to_start.is_empty() {
                inner.worker_running.store(false, Ordering::SeqCst);
                break;
            }
        }

        for job in to_start {
            let app = app.clone();
            let client = client.clone();
            let inner = inner.clone();

            std::thread::spawn(move || {
                let id = job.item.id.clone();
                let package_id = job.item.package_id.clone();
                let pending_dir = job.pending_dir.clone();
                let final_dir = job.final_dir.clone();

                println!(
                    "starting download:\n  remote: {}\n  user: {}\n  dir: {}\n  expected bytes: {}",
                    job.item.path,
                    job.item.username,
                    pending_dir.to_string_lossy(),
                    job.item.size
                );

                let download_result = client.download(
                    job.item.path.clone(),
                    job.item.username.clone(),
                    job.item.size,
                    pending_dir.to_string_lossy().to_string(),
                );

                let (_download, receiver) = match download_result {
                    Ok(value) => value,
                    Err(err) => {
                        let mut jobs = match inner.jobs.lock() {
                            Ok(jobs) => jobs,
                            Err(_) => return,
                        };

                        if let Some(stored) = jobs.get_mut(&id) {
                            stored.status = DownloadStatus::Failed;
                            stored.progress = 0.0;
                            stored.error = Some(err.to_string());
                            emit_download_event(&app, stored);
                        }

                        println!("download request failed: {err}");
                        return;
                    }
                };

                let started_at = Instant::now();
                let mut last_progress_at = Instant::now();
                let mut saw_progress = false;

                loop {
                    match receiver.recv_timeout(Duration::from_millis(500)) {
                        Ok(status) => {
                            let mut should_break = false;

                            {
                                let mut jobs = match inner.jobs.lock() {
                                    Ok(jobs) => jobs,
                                    Err(_) => return,
                                };

                                let Some(stored) = jobs.get_mut(&id) else {
                                    return;
                                };

                                match status {
                                    SoulseekDownloadStatus::Queued => {
                                        stored.status = DownloadStatus::Downloading;
                                        stored.progress = stored.progress.max(0.0);
                                        stored.error = None;
                                    }

                                    SoulseekDownloadStatus::InProgress {
                                        bytes_downloaded,
                                        total_bytes,
                                        speed_bytes_per_sec,
                                    } => {
                                        stored.status = DownloadStatus::Downloading;
                                        stored.downloaded_bytes = bytes_downloaded;

                                        if bytes_downloaded > 0 {
                                            saw_progress = true;
                                            last_progress_at = Instant::now();
                                        }

                                        stored.progress = if total_bytes > 0 {
                                            ((bytes_downloaded as f32 / total_bytes as f32) * 100.0)
                                                .clamp(0.0, 99.0)
                                        } else if stored.item.size > 0 {
                                            ((bytes_downloaded as f32 / stored.item.size as f32)
                                                * 100.0)
                                                .clamp(0.0, 99.0)
                                        } else {
                                            0.0
                                        };

                                        let now = Instant::now();
                                        let elapsed =
                                            now.duration_since(stored.last_speed_at).as_secs_f32();

                                        if elapsed >= 1.0 {
                                            let byte_delta = bytes_downloaded
                                                .saturating_sub(stored.last_speed_bytes);

                                            let calculated_speed = if byte_delta > 0 {
                                                byte_delta as f32 / elapsed
                                            } else {
                                                speed_bytes_per_sec as f32
                                            };

                                            // Smooth it slightly so it doesn't jump around as hard.
                                            stored.speed_bytes_per_sec =
                                                if stored.speed_bytes_per_sec > 0.0 {
                                                    (stored.speed_bytes_per_sec * 0.45)
                                                        + (calculated_speed * 0.55)
                                                } else {
                                                    calculated_speed
                                                };

                                            stored.last_speed_bytes = bytes_downloaded;
                                            stored.last_speed_at = now;
                                        }

                                        stored.error = None;
                                    }

                                    SoulseekDownloadStatus::Paused {
                                        bytes_downloaded,
                                        total_bytes,
                                    } => {
                                        stored.status = DownloadStatus::Paused;
                                        stored.speed_bytes_per_sec = 0.0;

                                        stored.progress = if total_bytes > 0 {
                                            ((bytes_downloaded as f32 / total_bytes as f32) * 100.0)
                                                .clamp(0.0, 99.0)
                                        } else {
                                            stored.progress
                                        };
                                    }

                                    SoulseekDownloadStatus::Completed => {
                                        stored.status = DownloadStatus::Done;
                                        stored.speed_bytes_per_sec = 0.0;
                                        stored.progress = 100.0;
                                        stored.error = None;
                                        should_break = true;
                                    }

                                    SoulseekDownloadStatus::Failed => {
                                        stored.status = DownloadStatus::Failed;
                                        stored.speed_bytes_per_sec = 0.0;
                                        stored.error = Some("download failed".to_string());
                                        should_break = true;
                                    }

                                    SoulseekDownloadStatus::TimedOut => {
                                        stored.status = DownloadStatus::Failed;
                                        stored.speed_bytes_per_sec = 0.0;
                                        stored.error = Some("download timed out".to_string());
                                        should_break = true;
                                    }
                                }

                                emit_download_event(&app, stored);
                            }

                            if should_break {
                                break;
                            }
                        }

                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            let mut jobs = match inner.jobs.lock() {
                                Ok(jobs) => jobs,
                                Err(_) => return,
                            };

                            let Some(stored) = jobs.get_mut(&id) else {
                                return;
                            };

                            if stored.status == DownloadStatus::Deleted {
                                return;
                            }

                            let waited_too_long_to_start =
                                !saw_progress && started_at.elapsed() >= Duration::from_secs(45);

                            let stalled_after_progress = saw_progress
                                && last_progress_at.elapsed() >= Duration::from_secs(90);

                            if waited_too_long_to_start {
                                stored.status = DownloadStatus::Failed;
                                stored.speed_bytes_per_sec = 0.0;
                                stored.error =
                                    Some("download did not start after 45 seconds".to_string());
                                emit_download_event(&app, stored);
                                return;
                            }

                            if stalled_after_progress {
                                stored.status = DownloadStatus::Failed;
                                stored.speed_bytes_per_sec = 0.0;
                                stored.error = Some("download stalled for 90 seconds".to_string());
                                emit_download_event(&app, stored);
                                return;
                            }
                        }

                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            let mut jobs = match inner.jobs.lock() {
                                Ok(jobs) => jobs,
                                Err(_) => return,
                            };

                            if let Some(stored) = jobs.get_mut(&id) {
                                if stored.status != DownloadStatus::Done {
                                    stored.status = DownloadStatus::Failed;
                                    stored.error =
                                        Some("download status channel disconnected".to_string());
                                    emit_download_event(&app, stored);
                                }
                            }

                            return;
                        }
                    }
                }

                let should_finalize = {
                    let jobs = match inner.jobs.lock() {
                        Ok(jobs) => jobs,
                        Err(_) => return,
                    };

                    let package_jobs: Vec<_> = jobs
                        .values()
                        .filter(|job| job.item.package_id == package_id)
                        .collect();

                    !package_jobs.is_empty()
                        && package_jobs
                            .iter()
                            .all(|job| job.status == DownloadStatus::Done)
                };

                if should_finalize {
                    match finalize_package_folder(&pending_dir, &final_dir) {
                        Ok(final_path) => {
                            println!(
                                "finished package {} -> {}",
                                package_id,
                                final_path.to_string_lossy()
                            );
                        }
                        Err(err) => {
                            println!("failed to finalize package {package_id}: {err}");
                        }
                    }
                }
            });
        }
    });
}

/* ----------------------------- EVENT HELPERS ----------------------------- */

fn emit_download_event(app: &AppHandle, job: &DownloadJob) {
    let _ = app.emit(
        "download-event",
        DownloadEvent {
            id: job.item.id.clone(),
            username: job.item.username.clone(),
            folder: job.item.folder.clone(),
            name: job.item.name.clone(),
            size: job.item.size,
            status: job.status.as_str().to_string(),
            progress: job.progress,
            speed_bytes_per_sec: job.speed_bytes_per_sec,
            error: job.error.clone(),
        },
    );
}
/* ----------------------------- FILE HELPERS ----------------------------- */

#[derive(Debug, Deserialize)]
struct EditLibraryTrackRequest {
    old_path: String,
    new_name: String,
}

#[derive(Debug, Deserialize)]
struct EditLibraryAlbumRequest {
    folder: String,
    album_name: String,
    tracks: Vec<EditLibraryTrackRequest>,
}

#[derive(Debug, Deserialize)]
struct LoadAlbumCoverRequest {
    folder: String,
}

fn find_best_folder_cover(folder: &Path) -> Result<Option<PathBuf>, String> {
    let entries = fs::read_dir(folder).map_err(|e| e.to_string())?;

    let mut candidates: Vec<(PathBuf, u64, i32)> = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        let Some(ext) = path
            .extension()
            .map(|ext| ext.to_string_lossy().to_lowercase())
        else {
            continue;
        };

        if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp") {
            continue;
        }

        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default();

        let priority = if file_name.contains("cover")
            || file_name.contains("folder")
            || file_name.contains("front")
            || file_name.contains("album")
        {
            2
        } else if file_name.contains("back")
            || file_name.contains("booklet")
            || file_name.contains("disc")
            || file_name.contains("cd")
        {
            0
        } else {
            1
        };

        let size = fs::metadata(&path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);

        candidates.push((path, size, priority));
    }

    candidates.sort_by(|a, b| {
        // Prefer likely front cover names, then bigger file size.
        b.2.cmp(&a.2).then(b.1.cmp(&a.1))
    });

    Ok(candidates.into_iter().next().map(|candidate| candidate.0))
}

#[tauri::command]
fn load_album_cover(req: LoadAlbumCoverRequest) -> Result<Option<String>, String> {
    let folder = PathBuf::from(req.folder);

    if !folder.exists() || !folder.is_dir() {
        return Ok(None);
    }

    let root = folder
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| folder.clone());

    // 1. Reuse cached extracted artwork if available.
    if let Some(existing) = find_cached_cover_for_folder(&root, &folder)? {
        return cover_file_to_data_url(&existing);
    }

    // 2. Prefer existing image files in the album folder.
    // This is fast and avoids parsing audio tags when cover.jpg/folder.png exists.
    if let Some(folder_cover) = find_best_folder_cover(&folder)? {
        return cover_file_to_data_url(&folder_cover);
    }

    // 3. Fall back to embedded cover art from audio files.
    let mut audio_files = Vec::new();
    collect_audio_files_shallow(&folder, &mut audio_files)?;

    for audio_path in audio_files {
        if let Some(cover_path) = extract_embedded_cover(&root, &audio_path)? {
            return cover_file_to_data_url(Path::new(&cover_path));
        }
    }

    Ok(None)
}

fn collect_audio_files_shallow(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| e.to_string())?;

    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();

        if path.is_file() && is_audio_file(&path) {
            files.push(path);
        }
    }

    files.sort_by(|a, b| {
        a.file_name()
            .unwrap_or_default()
            .cmp(b.file_name().unwrap_or_default())
    });

    Ok(())
}

fn find_cached_cover_for_folder(root: &Path, folder: &Path) -> Result<Option<PathBuf>, String> {
    let artwork_dir = root.join("_artwork");

    if !artwork_dir.exists() {
        return Ok(None);
    }

    let entries = fs::read_dir(&artwork_dir).map_err(|e| e.to_string())?;

    let folder_hash = hash_path(folder);

    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();

        let Some(stem) = path.file_stem().map(|s| s.to_string_lossy()) else {
            continue;
        };

        if stem.starts_with(&folder_hash) {
            return Ok(Some(path));
        }
    }

    Ok(None)
}

fn hash_path(path: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    path.to_string_lossy().hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

#[derive(Debug, Deserialize)]
struct DeleteLibraryAlbumRequest {
    folder: String,
}

#[tauri::command]
fn delete_library_album(req: DeleteLibraryAlbumRequest) -> Result<(), String> {
    let folder = PathBuf::from(req.folder);

    if !folder.exists() {
        return Ok(());
    }

    if !folder.is_dir() {
        return Err("library album path is not a folder".to_string());
    }

    fs::remove_dir_all(&folder).map_err(|e| format!("failed to delete album folder: {e}"))?;

    Ok(())
}

#[tauri::command]
fn read_audio_data_url(req: ReadAudioFileRequest) -> Result<String, String> {
    let path = PathBuf::from(req.path);

    if !path.exists() {
        return Err("audio file does not exist".to_string());
    }

    let data = fs::read(&path).map_err(|e| e.to_string())?;

    if data.is_empty() {
        return Err("audio file is empty".to_string());
    }

    let mime = audio_mime_for_path(&path);
    let encoded = general_purpose::STANDARD.encode(data);

    Ok(format!("data:{mime};base64,{encoded}"))
}

fn audio_mime_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .as_deref()
    {
        Some("mp3") => "audio/mpeg",
        Some("flac") => "audio/flac",
        Some("wav") => "audio/wav",
        Some("ogg") => "audio/ogg",
        Some("opus") => "audio/opus",
        Some("m4a") => "audio/x-m4a",
        Some("aac") => "audio/aac",
        _ => "audio/mpeg",
    }
}

fn cover_file_to_data_url(path: &Path) -> Result<Option<String>, String> {
    if !path.exists() {
        return Ok(None);
    }

    let data = fs::read(path).map_err(|e| e.to_string())?;

    if data.is_empty() {
        return Ok(None);
    }

    let mime = match path
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "image/jpeg",
    };

    let encoded = general_purpose::STANDARD.encode(data);

    Ok(Some(format!("data:{mime};base64,{encoded}")))
}

fn extract_embedded_cover(root: &Path, audio_path: &Path) -> Result<Option<String>, String> {
    if audio_path
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase() == "mp3")
        .unwrap_or(false)
    {
        if let Some(path) = extract_mp3_cover_with_id3(root, audio_path)? {
            return Ok(Some(path));
        }
    }

    extract_cover_with_lofty(root, audio_path)
}

fn extract_mp3_cover_with_id3(root: &Path, audio_path: &Path) -> Result<Option<String>, String> {
    let tag = match id3::Tag::read_from_path(audio_path) {
        Ok(tag) => tag,
        Err(err) => {
            println!(
                "id3 could not read tags for cover: {} ({})",
                audio_path.to_string_lossy(),
                err
            );
            return Ok(None);
        }
    };

    let Some(picture) = tag.pictures().next() else {
        println!(
            "id3 tags found but no embedded picture: {}",
            audio_path.to_string_lossy()
        );
        return Ok(None);
    };

    let data = &picture.data;

    if data.is_empty() {
        return Ok(None);
    }

    let extension = if picture.mime_type.contains("png") {
        "png"
    } else if picture.mime_type.contains("webp") {
        "webp"
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if data.starts_with(&[0xff, 0xd8, 0xff]) {
        "jpg"
    } else {
        "jpg"
    };

    let cover_path = write_cover_file(root, audio_path, data, extension)?;

    println!(
        "extracted mp3 cover:\n  audio: {}\n  cover: {}",
        audio_path.to_string_lossy(),
        cover_path
    );

    Ok(Some(cover_path))
}

fn extract_cover_with_lofty(root: &Path, audio_path: &Path) -> Result<Option<String>, String> {
    let tagged_file = match Probe::open(audio_path)
        .map_err(|e| format!("failed to open audio file for tags: {e}"))?
        .read()
    {
        Ok(file) => file,
        Err(err) => {
            println!(
                "lofty could not read tags for cover: {} ({})",
                audio_path.to_string_lossy(),
                err
            );
            return Ok(None);
        }
    };

    let tags = tagged_file.tags();

    if tags.is_empty() {
        println!("no tags found for: {}", audio_path.to_string_lossy());
        return Ok(None);
    }

    for tag in tags {
        let pictures = tag.pictures();

        if pictures.is_empty() {
            continue;
        }

        let Some(picture) = pictures.first() else {
            continue;
        };

        let data = picture.data();

        if data.is_empty() {
            continue;
        }

        let extension = if data.starts_with(b"\x89PNG\r\n\x1a\n") {
            "png"
        } else if data.starts_with(&[0xff, 0xd8, 0xff]) {
            "jpg"
        } else if data.starts_with(b"GIF") {
            "gif"
        } else if data.starts_with(b"RIFF") {
            "webp"
        } else {
            "jpg"
        };

        let cover_path = write_cover_file(root, audio_path, data, extension)?;

        println!(
            "extracted lofty cover:\n  audio: {}\n  cover: {}",
            audio_path.to_string_lossy(),
            cover_path
        );

        return Ok(Some(cover_path));
    }

    println!(
        "tags found but no embedded pictures: {}",
        audio_path.to_string_lossy()
    );
    Ok(None)
}

fn write_cover_file(
    root: &Path,
    audio_path: &Path,
    data: &[u8],
    extension: &str,
) -> Result<String, String> {
    let artwork_dir = root.join("_artwork");
    fs::create_dir_all(&artwork_dir).map_err(|e| e.to_string())?;

    let album_dir = audio_path.parent().unwrap_or(root);

    let folder_hash = hash_path(album_dir);
    let cover_path = artwork_dir.join(format!("{folder_hash}.{extension}"));

    if cover_path.exists() {
        return Ok(cover_path.to_string_lossy().to_string());
    }

    fs::write(&cover_path, data).map_err(|e| e.to_string())?;

    Ok(cover_path.to_string_lossy().to_string())
}

#[tauri::command]
fn edit_library_album(req: EditLibraryAlbumRequest) -> Result<(), String> {
    let old_folder = PathBuf::from(&req.folder);

    if !old_folder.exists() || !old_folder.is_dir() {
        return Err("album folder does not exist".to_string());
    }

    let parent = old_folder
        .parent()
        .ok_or_else(|| "album folder has no parent".to_string())?
        .to_path_buf();

    let new_album_name = sanitize_folder_name(&req.album_name);
    let new_folder = parent.join(new_album_name);

    let final_folder = if old_folder != new_folder {
        if new_folder.exists() {
            return Err("an album folder with that name already exists".to_string());
        }

        fs::rename(&old_folder, &new_folder)
            .map_err(|e| format!("failed to rename album folder: {e}"))?;

        new_folder
    } else {
        old_folder
    };

    for track in req.tracks {
        let old_track_path = PathBuf::from(&track.old_path);

        let old_file_name = old_track_path
            .file_name()
            .ok_or_else(|| "track path has no filename".to_string())?;

        let current_path = final_folder.join(old_file_name);

        if !current_path.exists() {
            continue;
        }

        let old_extension = current_path
            .extension()
            .map(|ext| ext.to_string_lossy().to_string())
            .unwrap_or_default();

        let mut new_name = sanitize_file_name(&track.new_name);

        let new_has_extension = Path::new(&new_name).extension().is_some();

        if !new_has_extension && !old_extension.is_empty() {
            new_name = format!("{new_name}.{old_extension}");
        }

        let new_path = final_folder.join(new_name);

        if current_path == new_path {
            continue;
        }

        if new_path.exists() {
            return Err(format!(
                "track already exists: {}",
                new_path
                    .file_name()
                    .map(|name| name.to_string_lossy())
                    .unwrap_or_default()
            ));
        }

        fs::rename(&current_path, &new_path)
            .map_err(|e| format!("failed to rename track: {e}"))?;
    }

    Ok(())
}

fn sanitize_file_name(name: &str) -> String {
    let invalid = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

    let mut cleaned: String = name
        .chars()
        .map(|ch| {
            if invalid.contains(&ch) || ch.is_control() {
                '_'
            } else {
                ch
            }
        })
        .collect();

    while cleaned.ends_with('.') {
        cleaned.pop();
    }

    let cleaned = cleaned.trim().to_string();

    if cleaned.is_empty() {
        "Unknown Track".to_string()
    } else {
        cleaned
    }
}

fn sanitize_folder_name(name: &str) -> String {
    let mut out = String::new();

    for ch in name.chars() {
        let invalid =
            matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || ch.is_control();

        if invalid {
            out.push('_');
        } else {
            out.push(ch);
        }
    }

    let trimmed = out.trim().trim_end_matches('.').to_string();

    if trimmed.is_empty() {
        "Unknown Album".to_string()
    } else {
        trimmed
    }
}

fn unique_destination_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }

    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let base_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "Download".to_string());

    for i in 2..1000 {
        let candidate = parent.join(format!("{base_name} ({i})"));

        if !candidate.exists() {
            return candidate;
        }
    }

    parent.join(format!("{base_name} copy"))
}

fn finalize_package_folder(pending_dir: &Path, final_dir: &Path) -> Result<PathBuf, String> {
    if !pending_dir.exists() {
        return Ok(final_dir.to_path_buf());
    }

    let final_target = unique_destination_path(final_dir);

    if let Some(parent) = final_target.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    fs::rename(pending_dir, &final_target)
        .map_err(|e| format!("failed to move pending folder into library: {e}"))?;

    Ok(final_target)
}

/* ----------------------------- SEARCH HELPERS ----------------------------- */

fn split_folder_and_name(path: &str) -> (String, String) {
    let normalized = path.replace("/", "\\");

    match normalized.rsplit_once("\\") {
        Some((folder, name)) => (folder.to_string(), name.to_string()),
        None => ("Unknown folder".to_string(), normalized),
    }
}

fn format_attributes_from_name(name: &str) -> String {
    let lower = name.to_lowercase();

    let looks_alac = lower.contains("alac")
        || lower.contains("apple lossless")
        || lower.contains("apple_lossless")
        || lower.contains("lossless m4a")
        || lower.contains("m4a lossless");

    let looks_aac = lower.contains("aac")
        || lower.contains("itunes")
        || lower.contains("m4a aac")
        || lower.contains("aac m4a");

    if lower.ends_with(".flac") {
        "flac · lossless".to_string()
    } else if lower.ends_with(".mp3") {
        "mp3".to_string()
    } else if lower.ends_with(".m4a") {
        if looks_alac {
            "m4a · ALAC".to_string()
        } else if looks_aac {
            "m4a · AAC".to_string()
        } else {
            "m4a · codec unknown".to_string()
        }
    } else if lower.ends_with(".aac") {
        "aac".to_string()
    } else if lower.ends_with(".wav") {
        "wav · lossless".to_string()
    } else if lower.ends_with(".aiff") || lower.ends_with(".aif") {
        "aiff · lossless".to_string()
    } else if lower.ends_with(".ogg") {
        "ogg".to_string()
    } else if lower.ends_with(".opus") {
        "opus".to_string()
    } else {
        "".to_string()
    }
}

fn duration_from_attributes(attributes: &HashMap<u32, u32>) -> Option<u32> {
    attributes.get(&1).copied()
}

fn flatten_search_results(results: Vec<SearchResult>) -> Vec<UiSearchResult> {
    let mut out = Vec::new();

    for result in results {
        for file in result.files {
            let (folder, name) = split_folder_and_name(&file.name);

            out.push(UiSearchResult {
                username: file.username,
                folder,
                attributes: format_attributes_from_name(&name),
                duration_seconds: duration_from_attributes(&file.attribs),
                name,
                size: file.size,
            });
        }
    }

    out
}
