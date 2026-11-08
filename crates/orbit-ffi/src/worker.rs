//! The data thread. Owns the SQLite store and does all blocking file work, so the host's main
//! thread never waits on disk. Requests run in order. A search whose sequence number is already
//! stale is skipped before it touches the database.

use crate::rows::*;
use orbit_data::{ClipKind, NewClip, RetentionPolicy, Snippet, SnippetInput, Store, UsageRecord};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

pub const SCOPE_ROOT: u32 = 0;
pub const SCOPE_NOTES: u32 = 1;
pub const SCOPE_SNIPPETS: u32 = 2;
pub const SCOPE_CLIPBOARD: u32 = 3;

pub const PURPOSE_INSERT: u32 = 0;
pub const PURPOSE_COPY: u32 = 1;
pub const PURPOSE_EXPAND: u32 = 2;
pub const PURPOSE_EDIT: u32 = 3;

pub enum Req {
    Search { seq: u64, scope: u32, query: String, limit: usize },
    LoadUsage,
    RecordUsage(String),
    NoteOpen { id: i64 },
    /// Autosave. `token` identifies the editor, so its first save creates the note and later
    /// ones update it.
    NoteSave { token: u64, id: Option<i64>, body: String },
    NoteDeleteIfEmpty { id: i64 },
    NoteSetPinned { id: i64, pinned: bool },
    NoteTrash { id: i64 },
    NoteRestoreLast,
    NoteCopy { id: i64 },
    SnippetGet { id: i64, purpose: u32, erase: u64 },
    SnippetSave { token: u64, id: Option<i64>, input: SnippetInput },
    SnippetDelete { id: i64 },
    SnippetTriggers,
    ClipAdd { clip: NewClip, image: Option<Vec<u8>> },
    ClipGet { id: i64, purpose: u32 },
    ClipSetPinned { id: i64, pinned: bool },
    ClipDelete { id: i64 },
    ClipClear,
    SetClipPolicy(RetentionPolicy),
    SetClipPersist(bool),
    Export { settings_json: String, base_dir: PathBuf, stamp: String },
    Import { path: PathBuf },
    Shed,
    Counts,
    /// Remove everything Orbit stored, then stop. Sent only after the user has confirmed.
    DeleteLocalData,
    /// Replies once every earlier request is finished. Used before a clean exit.
    Shutdown,
}

pub enum Resp {
    Results { seq: u64, scope: u32, query: String, rows: Vec<Row> },
    Usage(Vec<UsageRecord>),
    NoteBody { id: i64, body: String },
    NoteSaved { token: u64, id: i64 },
    NoteText { text: String },
    NoteRestored,
    Snippet { snippet: Snippet, purpose: u32, erase: u64 },
    SnippetSaved { token: u64, id: i64, conflict: Option<String> },
    Triggers(Vec<(String, i64)>),
    Clip { kind: ClipKind, text: String, blob: String, purpose: u32 },
    Changed,
    Exported(Result<PathBuf, String>),
    Imported(Result<String, String>),
    Counts { notes: i64, snippets: i64, clips: i64, sqlite_bytes: i64 },
    Failed(String),
    Finished,
}

pub struct WorkerHandles {
    pub tx: Sender<Req>,
    pub rx: Receiver<Resp>,
    pub latest_seq: Arc<AtomicU64>,
}

pub fn start(dir: PathBuf, persist: bool, policy: RetentionPolicy, wake: impl Fn() + Send + 'static) -> Option<WorkerHandles> {
    let (req_tx, req_rx) = channel::<Req>();
    let (resp_tx, resp_rx) = channel::<Resp>();
    let latest_seq = Arc::new(AtomicU64::new(0));
    let latest = latest_seq.clone();
    std::thread::Builder::new()
        .name("orbit-data".into())
        .stack_size(1024 * 1024)
        .spawn(move || {
            lower_priority();
            run(dir, persist, policy, req_rx, resp_tx, latest, wake);
        })
        .ok()?;
    Some(WorkerHandles { tx: req_tx, rx: resp_rx, latest_seq })
}

#[cfg(target_os = "macos")]
fn lower_priority() {
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(class: u32, relative: i32) -> i32;
    }
    const QOS_CLASS_UTILITY: u32 = 0x11;
    unsafe {
        pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0);
    }
}

#[cfg(not(target_os = "macos"))]
fn lower_priority() {}

fn run(dir: PathBuf, persist: bool, mut policy: RetentionPolicy, rx: Receiver<Req>, tx: Sender<Resp>, latest: Arc<AtomicU64>, wake: impl Fn()) {
    let reply = |response: Resp| {
        if tx.send(response).is_ok() {
            wake();
        }
    };
    let mut store = match Store::open(&dir, persist) {
        Ok(store) => store,
        Err(error) => {
            // No user data here: this is a storage error message.
            reply(Resp::Failed(format!("Could not open Orbit's database: {error}")));
            // Keep answering so nothing waits forever. Launching apps does not need the store.
            for request in rx {
                if matches!(request, Req::Shutdown) {
                    reply(Resp::Finished);
                    break;
                }
            }
            return;
        }
    };
    let _ = store.purge_trash(30);
    if let Ok(blobs) = store.apply_clip_retention(&policy) {
        remove_blobs(&dir, &blobs);
    }
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
            Req::NoteSave { token, id, body } => {
                let known = id.or_else(|| note_tokens.get(&token).copied());
                let result = match known {
                    Some(existing) => store.save_note(existing, &body).map(|_| existing),
                    None if body.trim().is_empty() => continue,
                    None => store.create_note(&body),
                };
                match result {
                    Ok(saved) => {
                        note_tokens.insert(token, saved);
                        reply(Resp::NoteSaved { token, id: saved });
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
            Req::SnippetGet { id, purpose, erase } => match store.snippet(id) {
                Ok(Some(snippet)) => reply(Resp::Snippet { snippet, purpose, erase }),
                Ok(None) => reply(Resp::Failed("That snippet no longer exists".into())),
                Err(error) => reply(Resp::Failed(format!("Could not read snippet: {error}"))),
            },
            Req::SnippetSave { token, id, input } => {
                let conflict = store.trigger_conflict(&input.trigger, id.unwrap_or(0)).ok().flatten();
                let result = match id {
                    Some(existing) => store.update_snippet(existing, &input).map(|_| existing),
                    None => store.create_snippet(&input),
                };
                match result {
                    Ok(saved) => {
                        reply(Resp::SnippetSaved { token, id: saved, conflict });
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
            Req::ClipAdd { mut clip, image } => {
                if let Some(bytes) = image {
                    let name = format!("{:016x}.png", clip.content_hash.unwrap_or(0));
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
                if let Ok(Some((kind, text, blob))) = store.clip_content(id) {
                    let blob = if blob.is_empty() { blob } else { dir.join("clipboard-images").join(blob).to_string_lossy().into_owned() };
                    reply(Resp::Clip { kind, text, blob, purpose });
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
            Req::Export { settings_json, base_dir, stamp } => {
                reply(Resp::Exported(export(&store, &settings_json, &base_dir, &stamp)))
            }
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
                reply(Resp::Finished);
                break;
            }
            Req::Shutdown => {
                store.close_maintenance();
                reply(Resp::Finished);
                break;
            }
        }
    }
}

/// Removes what Orbit stored, by name. It never empties the folder as a whole, since `--data-dir`
/// can point anywhere. `hidutil-journal` stays if it's still there, because that means the Caps
/// Lock mapping couldn't be removed and the next start has to try again.
fn delete_local_files(dir: &std::path::Path) {
    orbit_data::remove_database_files(dir);
    for file in ["settings.json", "settings.damaged.json", crate::engine::INDEX_FILE, "instance.lock", "crash.log"] {
        let _ = std::fs::remove_file(dir.join(file));
    }
    for folder in ["icons", "clipboard-images"] {
        let _ = std::fs::remove_dir_all(dir.join(folder));
    }
    let _ = std::fs::remove_dir(dir); // only if that left it empty
}

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn search(store: &Store, scope: u32, query: &str, limit: usize) -> orbit_data::Result<Vec<Row>> {
    let q = query.trim();
    let now = now_unix();
    let mut rows = Vec::new();
    let snippet = |s: Snippet| snippet_row(s.id, s.name, &s.trigger, &s.content);
    match scope {
        SCOPE_ROOT => {
            if q.chars().count() >= 2 {
                rows.extend(store.search_notes(q, limit)?.into_iter().map(|n| note_row(n, now)));
                rows.extend(store.search_snippets(q, limit)?.into_iter().map(snippet));
            }
        }
        SCOPE_NOTES => {
            let notes = if q.is_empty() { store.recent_notes(limit)? } else { store.search_notes(q, limit)? };
            rows.extend(notes.into_iter().map(|n| note_row(n, now)));
        }
        SCOPE_SNIPPETS => {
            let snippets = if q.is_empty() { store.recent_snippets(limit)? } else { store.search_snippets(q, limit)? };
            rows.extend(snippets.into_iter().map(snippet));
        }
        _ => {
            let clips = if q.is_empty() { store.recent_clips(limit)? } else { store.search_clips(q, limit)? };
            rows.extend(clips.into_iter().map(|c| clip_row(c, now)));
        }
    }
    Ok(rows)
}

fn remove_blobs(dir: &Path, names: &[String]) {
    for name in names {
        // Names come from our own database, but never follow anything that isn't a bare file name.
        if !name.contains(['/', '\\']) && !name.contains("..") {
            let _ = std::fs::remove_file(dir.join("clipboard-images").join(name));
        }
    }
}

pub fn sanitize_file_name(title: &str) -> String {
    let mut out: String = title
        .chars()
        .map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .take(60)
        .collect();
    out = out.trim().trim_end_matches('.').to_string();
    if out.is_empty() { "note".into() } else { out }
}

fn export(store: &Store, settings_json: &str, base: &Path, stamp: &str) -> Result<PathBuf, String> {
    let bundle = store.export_bundle().map_err(|e| e.to_string())?;
    let folder = base.join(format!("Orbit Export {}", sanitize_file_name(stamp)));
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

/// FNV-1a. The identity of an image clip, and its file name.
pub fn image_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_cannot_escape_the_folder() {
        assert_eq!(sanitize_file_name("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(sanitize_file_name("   ...   "), "note");
    }
}
