//! The data thread. Owns the SQLite store and does all file reads, so the launcher's thread
//! never blocks on disk. Requests are handled in order. A search whose sequence number is
//! already stale is skipped before it touches the database.

use crate::gfx::downscale_premultiplied;
use crate::indexer::{ICON_BYTES, ICON_SIZE, icon_path};
use crate::util::SendHwnd;
use orbit_data::{ClipItem, ClipKind, Formats, NewClip, NoteSummary, RetentionPolicy, Snippet, SnippetInput, Store, UsageRecord};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

/// More than anyone keeps, and enough that the list in Settings never needs paging.
const SNIPPET_LIST_LIMIT: usize = 5000;
/// More text than the clipboard's preview has room for. The counts still cover all of it.
const PREVIEW_TEXT_CHARS: usize = 2000;
/// A longer list of copied files isn't looked up one by one for its size.
const PREVIEW_SIZED_FILES: usize = 50;

/// What the clipboard's side panel shows for the selected item. Only that one item is loaded, and
/// it's let go when the launcher hides.
pub struct ClipPreview {
    pub id: i64,
    pub item: ClipItem,
    pub body: PreviewBody,
}

pub enum PreviewBody {
    /// The start of the text, with counts over all of it.
    Text { shown: String, characters: usize, words: usize, lines: usize },
    /// Scaled down to fit, never up, as premultiplied BGRA.
    Image { width: usize, height: usize, pixels: Vec<u8> },
    /// Each path, and what they come to where that could be read quickly.
    Files { paths: Vec<String>, bytes: Option<u64> },
    /// Nothing to show, such as a picture in a format Orbit doesn't read.
    Nothing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataScope {
    Root,
    Notes,
    Snippets,
    Clipboard,
}

/// What a fetched snippet or clip is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Insert,
    Copy,
    /// Expanding a typed trigger: `trigger_chars` characters must be erased first.
    Expand { trigger_chars: usize },
    Edit,
    /// Paste a clip's text without its formatting.
    InsertPlain,
    /// Open a clip in the app Windows uses for it. A picture comes back as a PNG file's name.
    Open,
}

pub enum Req {
    Search { seq: u64, scope: DataScope, query: String, limit: usize },
    LoadIcon { hash: u64, size: usize },
    /// A clipboard image's thumbnail, answered as an icon.
    LoadClipThumb { id: i64, hash: u64, size: usize },
    /// A quicklink's website icon, answered as an icon.
    LoadFavicon { key: u64, hash: u64, size: usize },
    /// What the clipboard's side panel shows. Skipped if another item was selected since.
    ClipPreview { id: i64, max_width: usize, max_height: usize },
    LoadUsage,
    RecordUsage(String),
    NoteOpen { id: i64 },
    /// Autosave. `token` identifies the editor window, so its first save creates the note and
    /// later ones update it.
    NoteSave { token: u64, id: Option<i64>, body: String, editor: SendHwnd },
    NoteDeleteIfEmpty { id: i64 },
    NoteSetPinned { id: i64, pinned: bool },
    NoteTrash { id: i64 },
    NoteRestoreLast,
    /// Remove everything Orbit stored, then stop. Sent only after the user has confirmed.
    DeleteLocalData,
    NoteCopy { id: i64 },
    SnippetGet { id: i64, purpose: Purpose },
    SnippetSave { id: Option<i64>, input: SnippetInput, editor: SendHwnd },
    SnippetDelete { id: i64 },
    SnippetTriggers,
    /// Every snippet, for the list in Settings.
    SnippetList,
    ClipAdd { clip: NewClip, image: Option<Vec<u8>> },
    ClipGet { id: i64, purpose: Purpose },
    ClipSetPinned { id: i64, pinned: bool },
    ClipDelete { id: i64 },
    ClipClear,
    SetClipPolicy(RetentionPolicy),
    SetClipPersist(bool),
    Export { settings_json: String },
    Import { path: PathBuf },
    Shed,
    Counts,
    /// Replies once every earlier request is finished. Used before a clean exit.
    Shutdown,
}

pub enum DataRow {
    Note(NoteSummary),
    Snippet { id: i64, name: String, trigger: String, preview: String },
    Clip(ClipItem),
}

pub enum Resp {
    Results { seq: u64, scope: DataScope, query: String, rows: Vec<DataRow> },
    Icon { hash: u64, size: usize, pixels: Option<Vec<u8>> },
    Usage(Vec<UsageRecord>),
    NoteBody { id: i64, body: String },
    NoteSaved { token: u64, id: i64, editor: SendHwnd },
    NoteText { text: String },
    NoteRestored,
    Snippet { snippet: Snippet, purpose: Purpose },
    SnippetSaved { id: i64, conflict: Option<String>, editor: SendHwnd },
    Triggers(Vec<(String, i64)>),
    SnippetList(Vec<DataRow>),
    Clip { kind: ClipKind, text: String, blob: String, purpose: Purpose, formats: Formats },
    ClipPreview(Box<ClipPreview>),
    Changed,
    Exported(Result<PathBuf, String>),
    Imported(Result<String, String>),
    Counts { notes: i64, snippets: i64, clips: i64, sqlite_bytes: i64 },
    Failed(String),
    Finished,
}

pub struct Worker {
    pub tx: Sender<Req>,
    pub rx: Receiver<Resp>,
    /// The latest search. The thread skips searches older than this.
    pub latest_seq: Arc<AtomicU64>,
    /// The clip whose preview is wanted. Holding an arrow key down asks for many, and only the
    /// last one is worth reading.
    pub latest_preview: Arc<AtomicI64>,
}

impl Worker {
    pub fn send(&self, request: Req) {
        let _ = self.tx.send(request);
    }

    pub fn search(&self, seq: u64, scope: DataScope, query: String, limit: usize) {
        self.latest_seq.store(seq, Ordering::Relaxed);
        self.send(Req::Search { seq, scope, query, limit });
    }

    pub fn clip_preview(&self, id: i64, max_width: usize, max_height: usize) {
        self.latest_preview.store(id, Ordering::Relaxed);
        self.send(Req::ClipPreview { id, max_width, max_height });
    }
}

pub fn start(dir: PathBuf, persist_clipboard: bool, policy: RetentionPolicy, main: SendHwnd, message: u32) -> Worker {
    let (req_tx, req_rx) = channel::<Req>();
    let (resp_tx, resp_rx) = channel::<Resp>();
    let latest_seq = Arc::new(AtomicU64::new(0));
    let latest_preview = Arc::new(AtomicI64::new(0));
    let latest = (latest_seq.clone(), latest_preview.clone());
    let _ = std::thread::Builder::new()
        .name("orbit-data".into())
        .stack_size(1024 * 1024)
        .spawn(move || {
            below_normal_priority();
            run(dir, persist_clipboard, policy, req_rx, resp_tx, latest, main, message);
        });
    Worker { tx: req_tx, rx: resp_rx, latest_seq, latest_preview }
}

fn below_normal_priority() {
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    dir: PathBuf,
    persist_clipboard: bool,
    mut policy: RetentionPolicy,
    rx: Receiver<Req>,
    tx: Sender<Resp>,
    (latest, latest_preview): (Arc<AtomicU64>, Arc<AtomicI64>),
    main: SendHwnd,
    message: u32,
) {
    let reply = |response: Resp| {
        if tx.send(response).is_ok() {
            main.post(message, 0, 0);
        }
    };
    let mut store = match Store::open(&dir, persist_clipboard) {
        Ok(store) => store,
        Err(error) => {
            // No secrets here: this is a storage error message.
            reply(Resp::Failed(format!("Could not open Orbit's database: {error}")));
            // Keep answering so nothing waits forever. Launching apps does not need the store.
            for request in rx {
                match request {
                    Req::LoadIcon { hash, size } => {
                        reply(Resp::Icon { hash, size, pixels: load_icon(&dir, hash, size) })
                    }
                    Req::Shutdown => {
                        reply(Resp::Finished);
                        break;
                    }
                    _ => {}
                }
            }
            return;
        }
    };
    let _ = store.purge_trash(30);
    let _ = store.apply_clip_retention(&policy).map(|blobs| remove_blobs(&dir, &blobs));
    let mut note_tokens: HashMap<u64, i64> = HashMap::new();

    for request in rx.iter() {
        match request {
            Req::Search { seq, scope, query, limit } => {
                if seq < latest.load(Ordering::Relaxed) {
                    continue; // a newer query is already on its way
                }
                match search(&store, scope, &query, limit) {
                    Ok(rows) => reply(Resp::Results { seq, scope, query, rows }),
                    Err(error) => reply(Resp::Failed(format!("Search failed: {error}"))),
                }
            }
            Req::LoadIcon { hash, size } => {
                reply(Resp::Icon { hash, size, pixels: load_icon(&dir, hash, size) })
            }
            Req::LoadClipThumb { id, hash, size } => {
                let pixels = match store.clip_content(id) {
                    Ok(Some((ClipKind::Image, _, blob))) => std::fs::read(dir.join("clipboard-images").join(blob))
                        .ok()
                        .and_then(|dib| crate::gfx::dib_thumbnail(&dib, size)),
                    _ => None,
                };
                reply(Resp::Icon { hash, size, pixels });
            }
            Req::LoadFavicon { key, hash, size } => {
                let pixels = std::fs::read(crate::favicon::icon_file(&dir, key))
                    .ok()
                    .filter(|pixels| pixels.len() == ICON_BYTES)
                    .map(|pixels| downscale_premultiplied(&pixels, ICON_SIZE, size));
                reply(Resp::Icon { hash, size, pixels });
            }
            Req::ClipPreview { id, max_width, max_height } => {
                if latest_preview.load(Ordering::Relaxed) != id {
                    continue; // the selection has already moved on
                }
                if let Some(preview) = clip_preview(&store, &dir, id, max_width, max_height) {
                    reply(Resp::ClipPreview(Box::new(preview)));
                }
            }
            Req::LoadUsage => {
                if let Ok(records) = store.usage_records(2000) {
                    reply(Resp::Usage(records));
                }
            }
            Req::RecordUsage(key) => {
                let _ = store.record_usage(&key);
            }
            Req::NoteOpen { id } => match store.note_body(id) {
                Ok(Some(body)) => reply(Resp::NoteBody { id, body }),
                Ok(None) => reply(Resp::Failed("That note no longer exists".into())),
                Err(error) => reply(Resp::Failed(format!("Could not read note: {error}"))),
            },
            Req::NoteSave { token, id, body, editor } => {
                let known = id.or_else(|| note_tokens.get(&token).copied());
                let result = match known {
                    Some(existing) => store.save_note(existing, &body).map(|_| existing),
                    None if body.trim().is_empty() => continue,
                    None => store.create_note(&body),
                };
                match result {
                    Ok(saved) => {
                        note_tokens.insert(token, saved);
                        reply(Resp::NoteSaved { token, id: saved, editor });
                    }
                    Err(error) => reply(Resp::Failed(format!("Could not save note: {error}"))),
                }
            }
            Req::NoteDeleteIfEmpty { id } => {
                if store.note_body(id).ok().flatten().is_some_and(|b| b.trim().is_empty()) {
                    let _ = store.trash_note(id);
                }
            }
            Req::NoteSetPinned { id, pinned } => {
                let _ = store.set_note_pinned(id, pinned);
                reply(Resp::Changed);
            }
            Req::NoteTrash { id } => {
                let _ = store.trash_note(id);
                reply(Resp::Changed);
            }
            Req::NoteRestoreLast => {
                if let Ok(Some(id)) = store.recently_trashed_note() {
                    let _ = store.restore_note(id);
                    reply(Resp::NoteRestored);
                }
            }
            Req::NoteCopy { id } => {
                if let Ok(Some(text)) = store.note_body(id) {
                    reply(Resp::NoteText { text });
                }
            }
            Req::SnippetGet { id, purpose } => match store.snippet(id) {
                Ok(Some(snippet)) => reply(Resp::Snippet { snippet, purpose }),
                Ok(None) => reply(Resp::Failed("That snippet no longer exists".into())),
                Err(error) => reply(Resp::Failed(format!("Could not read snippet: {error}"))),
            },
            Req::SnippetSave { id, input, editor } => {
                let conflict = store.trigger_conflict(&input.trigger, id.unwrap_or(0)).ok().flatten();
                let result = match id {
                    Some(existing) => store.update_snippet(existing, &input).map(|_| existing),
                    None => store.create_snippet(&input),
                };
                match result {
                    Ok(saved) => {
                        reply(Resp::SnippetSaved { id: saved, conflict, editor });
                        if let Ok(triggers) = store.snippet_triggers() {
                            reply(Resp::Triggers(triggers));
                        }
                    }
                    Err(error) => reply(Resp::Failed(format!("Could not save snippet: {error}"))),
                }
            }
            Req::SnippetDelete { id } => {
                let _ = store.delete_snippet(id);
                reply(Resp::Changed);
                if let Ok(triggers) = store.snippet_triggers() {
                    reply(Resp::Triggers(triggers));
                }
            }
            Req::SnippetTriggers => {
                if let Ok(triggers) = store.snippet_triggers() {
                    reply(Resp::Triggers(triggers));
                }
            }
            Req::SnippetList => match search(&store, DataScope::Snippets, "", SNIPPET_LIST_LIMIT) {
                Ok(rows) => reply(Resp::SnippetList(rows)),
                Err(error) => reply(Resp::Failed(format!("Could not list snippets: {error}"))),
            },
            Req::ClipAdd { clip, image } => {
                let mut clip = clip;
                if let Some(bytes) = image {
                    let name = format!("{:016x}.dib", clip.content_hash.unwrap_or(0));
                    let images = dir.join("clipboard-images");
                    let _ = std::fs::create_dir_all(&images);
                    if std::fs::write(images.join(&name), &bytes).is_err() {
                        continue;
                    }
                    clip.blob_path = name;
                }
                if let Ok((_, purged)) = store.add_clip(&clip, &policy) {
                    remove_blobs(&dir, &purged);
                    reply(Resp::Changed);
                }
            }
            Req::ClipGet { id, purpose } => {
                if let Ok(Some((kind, text, mut blob))) = store.clip_content(id) {
                    let formats = match purpose {
                        Purpose::Insert | Purpose::Copy => store.clip_formats(id).unwrap_or_default(),
                        _ => Formats::default(),
                    };
                    if purpose == Purpose::Open && kind == ClipKind::Image {
                        blob = png_for(&dir, &blob).unwrap_or_default();
                    } else if kind == ClipKind::Image {
                        // Made once and kept, so pasting can offer apps a PNG beside the bitmap.
                        let _ = png_for(&dir, &blob);
                    }
                    reply(Resp::Clip { kind, text, blob, purpose, formats });
                }
            }
            Req::ClipSetPinned { id, pinned } => {
                let _ = store.set_clip_pinned(id, pinned);
                reply(Resp::Changed);
            }
            Req::ClipDelete { id } => {
                if let Ok(Some(path)) = store.delete_clip(id) {
                    remove_blobs(&dir, &[path]);
                }
                reply(Resp::Changed);
            }
            Req::ClipClear => {
                if let Ok(blobs) = store.clear_clips(false) {
                    remove_blobs(&dir, &blobs);
                }
                reply(Resp::Changed);
            }
            Req::SetClipPolicy(new_policy) => {
                policy = new_policy;
                if let Ok(blobs) = store.apply_clip_retention(&policy) {
                    remove_blobs(&dir, &blobs);
                }
            }
            Req::SetClipPersist(persist) => {
                let _ = store.set_clipboard_persistence(persist);
            }
            Req::Export { settings_json } => reply(Resp::Exported(export(&store, &settings_json))),
            Req::Import { path } => reply(Resp::Imported(import(&store, &path))),
            Req::Shed => store.shed_memory(),
            Req::Counts => {
                if let Ok((notes, snippets, clips)) = store.counts() {
                    reply(Resp::Counts { notes, snippets, clips, sqlite_bytes: Store::sqlite_memory_bytes() });
                }
            }
            Req::DeleteLocalData => {
                drop(store); // the databases can't be removed while they're open
                delete_local_files(&dir);
                FINISHED.store(true, std::sync::atomic::Ordering::SeqCst);
                reply(Resp::Finished);
                break;
            }
            Req::Shutdown => {
                store.close_maintenance();
                FINISHED.store(true, std::sync::atomic::Ordering::SeqCst);
                reply(Resp::Finished);
                break;
            }
        }
    }
}

fn search(store: &Store, scope: DataScope, query: &str, limit: usize) -> orbit_data::Result<Vec<DataRow>> {
    let q = query.trim();
    let mut rows = Vec::new();
    let snippet_row = |s: Snippet| DataRow::Snippet {
        id: s.id,
        preview: preview(&s.content),
        name: s.name,
        trigger: s.trigger,
    };
    match scope {
        DataScope::Root => {
            if q.chars().count() >= 2 {
                rows.extend(store.search_notes(q, limit)?.into_iter().map(DataRow::Note));
                rows.extend(store.search_snippets(q, limit)?.into_iter().map(snippet_row));
            }
        }
        DataScope::Notes => {
            let notes = if q.is_empty() { store.recent_notes(limit)? } else { store.search_notes(q, limit)? };
            rows.extend(notes.into_iter().map(DataRow::Note));
        }
        DataScope::Snippets => {
            let snippets = if q.is_empty() { store.recent_snippets(limit)? } else { store.search_snippets(q, limit)? };
            rows.extend(snippets.into_iter().map(snippet_row));
        }
        DataScope::Clipboard => {
            let clips = if q.is_empty() { store.recent_clips(limit)? } else { store.search_clips(q, limit)? };
            rows.extend(clips.into_iter().map(DataRow::Clip));
        }
    }
    Ok(rows)
}

fn clip_preview(store: &Store, dir: &Path, id: i64, max_width: usize, max_height: usize) -> Option<ClipPreview> {
    let item = store.clip_item(id).ok()??;
    let (kind, text, blob) = store.clip_content(id).ok()??;
    let body = match kind {
        ClipKind::Image if is_bare_name(&blob) => std::fs::read(dir.join("clipboard-images").join(&blob))
            .ok()
            .and_then(|dib| crate::gfx::dib_preview(&dib, max_width, max_height))
            .map_or(PreviewBody::Nothing, |(width, height, pixels)| PreviewBody::Image { width, height, pixels }),
        ClipKind::Image => PreviewBody::Nothing,
        ClipKind::Files => {
            let paths: Vec<String> = text.lines().filter(|line| !line.trim().is_empty()).map(str::to_string).collect();
            let bytes = files_size(&paths);
            PreviewBody::Files { paths, bytes }
        }
        ClipKind::Text | ClipKind::Url => text_preview(&text),
    };
    Some(ClipPreview { id, item, body })
}

fn text_preview(text: &str) -> PreviewBody {
    PreviewBody::Text {
        shown: text.chars().take(PREVIEW_TEXT_CHARS).collect(),
        characters: text.chars().count(),
        words: text.split_whitespace().count(),
        lines: text.lines().count().max(1),
    }
}

/// What copied files add up to. `None` for a folder, a network path, which could keep the data
/// thread waiting, a file that's gone, or too many to look up.
fn files_size(paths: &[String]) -> Option<u64> {
    if paths.is_empty() || paths.len() > PREVIEW_SIZED_FILES {
        return None;
    }
    let mut total = 0;
    for path in paths {
        if path.starts_with("\\\\") || path.starts_with("//") {
            return None;
        }
        let meta = std::fs::metadata(path).ok()?;
        if !meta.is_file() {
            return None;
        }
        total += meta.len();
    }
    Some(total)
}

/// Names come from our own database, but never follow anything that isn't a bare file name.
fn is_bare_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\']) && !name.contains("..")
}

fn preview(text: &str) -> String {
    let mut out = String::with_capacity(64);
    for (i, c) in text.chars().enumerate() {
        if i == 64 {
            out.push('…');
            break;
        }
        out.push(if c.is_control() { ' ' } else { c });
    }
    out
}

pub fn load_icon(dir: &Path, hash: u64, size: usize) -> Option<Vec<u8>> {
    let pixels = std::fs::read(icon_path(dir, hash)).ok()?;
    if pixels.len() != ICON_BYTES {
        return None;
    }
    Some(downscale_premultiplied(&pixels, ICON_SIZE, size))
}

fn remove_blobs(dir: &Path, names: &[String]) {
    for name in names {
        if is_bare_name(name) {
            let file = dir.join("clipboard-images").join(name);
            // The copy made to open it goes with it.
            let _ = std::fs::remove_file(file.with_extension("png"));
            let _ = std::fs::remove_file(file);
        }
    }
}

/// The PNG beside a stored clipboard picture, made the first time it's opened. Its file name, or
/// `None` if the picture is gone or won't convert.
fn png_for(dir: &Path, blob: &str) -> Option<String> {
    if !is_bare_name(blob) {
        return None;
    }
    let source = dir.join("clipboard-images").join(blob);
    let png = source.with_extension("png");
    if !png.is_file() {
        let bytes = std::fs::read(&source).ok()?;
        if !crate::gfx::dib_to_png(&bytes, &png) {
            return None;
        }
    }
    png.file_name().map(|name| name.to_string_lossy().into_owned())
}

fn sanitize_file_name(title: &str) -> String {
    let mut out: String = title
        .chars()
        .map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .take(60)
        .collect();
    out = out.trim().trim_end_matches('.').to_string();
    if out.is_empty() { "note".into() } else { out }
}

/// Everything Orbit creates in its data folder, and nothing else that might be there.
fn delete_local_files(dir: &Path) {
    orbit_data::remove_database_files(dir);
    for file in ["settings.json", "settings.damaged.json", crate::indexer::INDEX_FILE, "timings.csv", "crash.log"] {
        let _ = std::fs::remove_file(dir.join(file));
    }
    for folder in ["icons", "clipboard-images", crate::favicon::FOLDER] {
        let _ = std::fs::remove_dir_all(dir.join(folder));
    }
    let _ = std::fs::remove_dir(dir); // only if that left it empty
}

fn export(store: &Store, settings_json: &str) -> Result<PathBuf, String> {
    let bundle = store.export_bundle().map_err(|e| e.to_string())?;
    let base = crate::util::documents_folder().unwrap_or_else(std::env::temp_dir);
    let (date, time) = crate::util::local_date_time();
    let name = format!("Orbit Export {date} {}", time.replace(':', ""));
    // Never into an export made earlier in the same minute.
    let folder = (1..)
        .map(|i| if i == 1 { base.join(&name) } else { base.join(format!("{name} ({i})")) })
        .find(|candidate| !candidate.exists())
        .unwrap_or_else(|| base.join(&name));
    let notes = folder.join("notes");
    std::fs::create_dir_all(&notes).map_err(|e| e.to_string())?;
    std::fs::write(folder.join("settings.json"), settings_json).map_err(|e| e.to_string())?;
    let json = serde_json::to_vec_pretty(&bundle).map_err(|e| e.to_string())?;
    std::fs::write(folder.join("orbit-export.json"), json).map_err(|e| e.to_string())?;
    for (i, note) in bundle.notes.iter().enumerate() {
        let title = orbit_data::title_from_body(&note.body);
        let name = format!("{:05} {}.md", i + 1, sanitize_file_name(&title));
        std::fs::write(notes.join(name), &note.body).map_err(|e| e.to_string())?;
    }
    Ok(folder)
}

fn import(store: &Store, path: &Path) -> Result<String, String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > orbit_data::MAX_IMPORT_BYTES {
        return Err("That file is larger than the import limit.".into());
    }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let summary = store.import_bundle(&text).map_err(|e| e.to_string())?;
    Ok(format!(
        "Imported {} notes and {} snippets. Skipped {} that already existed.",
        summary.notes_added,
        summary.snippets_added,
        summary.notes_skipped + summary.snippets_skipped
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_cannot_escape_the_folder() {
        assert_eq!(sanitize_file_name("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(sanitize_file_name("a:b*c?"), "a_b_c_");
        assert_eq!(sanitize_file_name("   ...   "), "note");
        assert_eq!(sanitize_file_name(""), "note");
    }

    #[test]
    fn previews_are_bounded() {
        assert_eq!(preview("a\nb"), "a b");
        assert_eq!(preview(&"x".repeat(100)).chars().count(), 65);
    }

    #[test]
    fn text_previews_count_all_of_it_but_hold_only_the_start() {
        let text = format!("one two\nthree {}", "é".repeat(PREVIEW_TEXT_CHARS * 2));
        let PreviewBody::Text { shown, characters, words, lines } = text_preview(&text) else { panic!() };
        assert_eq!(shown.chars().count(), PREVIEW_TEXT_CHARS);
        assert_eq!(characters, 14 + PREVIEW_TEXT_CHARS * 2);
        assert_eq!((words, lines), (4, 2));
        let PreviewBody::Text { lines, words, .. } = text_preview("") else { panic!() };
        assert_eq!((lines, words), (1, 0));
    }

    #[test]
    fn copied_files_are_sized_only_when_thats_quick() {
        let dir = std::env::temp_dir().join(format!("orbit-files-size-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, b"12345").unwrap();
        let path = file.to_string_lossy().to_string();
        assert_eq!(files_size(std::slice::from_ref(&path)), Some(5));
        assert_eq!(files_size(&[path.clone(), path.clone()]), Some(10));
        assert_eq!(files_size(&[dir.to_string_lossy().to_string()]), None, "a folder isn't walked");
        assert_eq!(files_size(&["\\\\server\\share\\a.txt".into()]), None, "nor is the network asked");
        assert_eq!(files_size(&[dir.join("gone.txt").to_string_lossy().to_string()]), None);
        assert_eq!(files_size(&vec![path; PREVIEW_SIZED_FILES + 1]), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_bare_names_are_followed() {
        assert!(is_bare_name("00ff.dib"));
        assert!(!is_bare_name(""));
        assert!(!is_bare_name("..\\x.dib"));
        assert!(!is_bare_name("a/b.dib"));
    }
}

static FINISHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Blocks until the data thread has done its final write, or the time runs out.
pub fn wait_for_finish(longest: std::time::Duration) {
    let start = std::time::Instant::now();
    while !FINISHED.load(std::sync::atomic::Ordering::SeqCst) && start.elapsed() < longest {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
