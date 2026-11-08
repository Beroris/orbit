//! Quicklink icons, asked of each link's own site and nowhere else. This is Orbit's only network
//! code, and one switch in Settings turns it off.
//!
//! A link's site is asked once: when the link is saved, or at start for a link that has never been
//! asked about. The answer, or the fact that there was none, is kept in the data folder, so nothing
//! is asked again after that. Requests carry no cookies and no Windows sign-in, wait at most a few
//! seconds, and read a bounded amount. Only the address's host is used: never its path or query,
//! which may hold something personal. Icons named on another site, such as an icon service, are
//! refused. The one exception: when the link's own site answers a request for its icon by sending
//! Orbit elsewhere, as Gmail sends it to Google's file server, that one step is followed, to a
//! public HTTPS address only.

use crate::indexer::{ICON_BYTES, ICON_SIZE};
use crate::util::SendHwnd;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

pub const FOLDER: &str = "link-icons";
/// Enough of a page to reach its `<head>`.
const PAGE_LIMIT: usize = 256 * 1024;
const ICON_LIMIT: usize = 1024 * 1024;
const TIMEOUT_MS: i32 = 5000;
const MOST_REDIRECTS: usize = 3;
/// The largest picture decoded. A bigger frame in an icon file is passed over for a smaller one.
const LARGEST_FRAME: u32 = 1024;

/// Off in Settings: anything queued or under way stops before it writes.
static ENABLED: AtomicBool = AtomicBool::new(true);
/// Hosts waiting to be asked, and whether a thread is already asking.
static QUEUE: Mutex<(Vec<(String, bool)>, bool)> = Mutex::new((Vec::new(), false));

/// The host of a web address, lower case, with any port. `None` for anything that isn't http(s),
/// or whose host is itself a blank to fill in.
pub fn host_of(url: &str) -> Option<String> {
    let lower = url.trim().to_ascii_lowercase();
    let rest = lower.strip_prefix("https://").or_else(|| lower.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    if host.is_empty() || host.contains(['{', '}', '\\', ' ']) {
        return None;
    }
    Some(host.to_string())
}

fn without_port(host: &str) -> &str {
    host.split(':').next().unwrap_or(host)
}

/// A name on the public internet. Local names, private addresses, names reserved for testing and
/// bare IPv6 addresses are never asked: a link to them says nothing a stranger should hear.
pub fn is_public(host: &str) -> bool {
    let name = without_port(host).trim_end_matches('.');
    if name.starts_with('[') || !name.contains('.') {
        return false;
    }
    const PRIVATE_ENDINGS: [&str; 9] = [".local", ".localhost", ".internal", ".lan", ".home", ".corp", ".home.arpa", ".invalid", ".test"];
    if PRIVATE_ENDINGS.iter().any(|ending| name.ends_with(ending)) || name.ends_with(".example") || name == "example.com" {
        return false;
    }
    let parts: Vec<u8> = name.split('.').filter_map(|part| part.parse().ok()).collect();
    if parts.len() == 4 && name.split('.').count() == 4 {
        let [a, b, ..] = [parts[0], parts[1], parts[2], parts[3]];
        let private = a == 10 || a == 127 || a == 0 || (a == 172 && (16..32).contains(&b)) || (a == 192 && b == 168) || (a == 169 && b == 254) || (a == 100 && (64..128).contains(&b));
        return !private && a < 224;
    }
    true
}

/// The same site: the host itself, or a name under it or above it, such as `www.` added or taken
/// away. Never anyone else's.
fn same_site(host: &str, other: &str) -> bool {
    let (a, b) = (without_port(host), without_port(other));
    a == b || (b.contains('.') && (a.ends_with(&format!(".{b}")) || b.ends_with(&format!(".{a}"))))
}

/// FNV-1a of the host, which names its file.
pub fn key(host: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in host.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Where a host's icon sits in the icon cache, away from apps and clipboard pictures.
pub fn row_hash(key: u64) -> u64 {
    0xFA71_0000_0000_0000 | (key & 0xFFFF_FFFF_FFFF)
}

/// The key of a link's icon, if it has a site Orbit would ask.
pub fn link_key(url: &str) -> Option<u64> {
    host_of(url).filter(|host| is_public(host)).map(|host| key(&host))
}

/// Named for how it's drawn, so icons kept from before logos filled their square are fetched again.
pub fn icon_file(dir: &Path, key: u64) -> PathBuf {
    dir.join(FOLDER).join(format!("{key:016x}-full.bgra"))
}

/// Where an icon drawn on a white tile, as earlier versions did, was kept.
fn tiled_icon_file(dir: &Path, key: u64) -> PathBuf {
    dir.join(FOLDER).join(format!("{key:016x}.bgra"))
}

/// Says the site was asked and had nothing usable, so it isn't asked again at every start.
fn none_file(dir: &Path, key: u64) -> PathBuf {
    dir.join(FOLDER).join(format!("{key:016x}.none"))
}

/// What a "no icon" file holds. One written by an earlier version, before Orbit could read SVG icons
/// or follow a site's own redirect for its icon, holds less, so a site marked then is asked once more.
const NONE_VERSION: &[u8] = b"3";

fn known_to_have_none(dir: &Path, key: u64) -> bool {
    std::fs::read(none_file(dir, key)).is_ok_and(|bytes| bytes == NONE_VERSION)
}

/// Where an `href` points, from a page at the root of `https://host/`.
fn resolve(href: &str, scheme: &str, host: &str) -> Option<String> {
    let href = href.trim();
    let lower = href.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        Some(href.to_string())
    } else if let Some(rest) = href.strip_prefix("//") {
        Some(format!("{scheme}://{rest}"))
    } else if href.starts_with('/') {
        Some(format!("{scheme}://{host}{href}"))
    } else if href.is_empty() || lower.contains(':') {
        None // data:, javascript: and the like
    } else {
        Some(format!("{scheme}://{host}/{href}"))
    }
}

/// The value of `name` in one tag's attributes, quoted or not.
fn attribute<'a>(tag: &'a str, lower: &str, name: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(at) = lower[from..].find(name).map(|at| at + from) {
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let after = lower[at + name.len()..].trim_start();
        if before_ok && after.starts_with('=') {
            let value_start = tag.len() - after.len() + 1;
            let value = tag[value_start..].trim_start();
            let start = tag.len() - value.len();
            return Some(match value.chars().next() {
                Some(quote @ ('"' | '\'')) => {
                    let inner = &tag[start + 1..];
                    &inner[..inner.find(quote).unwrap_or(inner.len())]
                }
                _ => &tag[start..start + value.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(value.len())],
            });
        }
        from = at + name.len();
    }
    None
}

/// The icons a page names in its `<link>` tags, best first: the biggest pictures, then any drawn
/// as SVG, which Direct2D reads but not always every part of.
pub fn icon_links(html: &str, scheme: &str, host: &str) -> Vec<String> {
    let lower_html = html.to_ascii_lowercase();
    let mut found: Vec<(u32, String)> = Vec::new();
    let mut drawn: Vec<String> = Vec::new();
    let mut from = 0;
    while let Some(at) = lower_html[from..].find("<link").map(|at| at + from) {
        let end = lower_html[at..].find('>').map_or(lower_html.len(), |end| at + end);
        let (tag, lower) = (&html[at..end], &lower_html[at..end]);
        from = end;
        let Some(rel) = attribute(tag, lower, "rel").map(str::to_ascii_lowercase) else { continue };
        let words: Vec<&str> = rel.split_whitespace().collect();
        let touch = words.iter().any(|word| word.starts_with("apple-touch-icon"));
        if !(touch || words.contains(&"icon")) {
            continue;
        }
        let Some(href) = attribute(tag, lower, "href") else { continue };
        let kind = attribute(tag, lower, "type").unwrap_or("").to_ascii_lowercase();
        if kind.contains("svg") || href.to_ascii_lowercase().split(['?', '#']).next().is_some_and(|path| path.ends_with(".svg")) {
            if let Some(url) = resolve(href, scheme, host) {
                drawn.push(url);
            }
            continue;
        }
        let sizes = attribute(tag, lower, "sizes").unwrap_or("");
        let size = sizes
            .split_whitespace()
            .filter_map(|size| size.split(['x', 'X']).next()?.parse::<u32>().ok())
            .max()
            .unwrap_or(if touch { 180 } else { 32 });
        if let Some(url) = resolve(href, scheme, host) {
            found.push((size.min(512), url));
        }
    }
    found.sort_by_key(|(size, _)| std::cmp::Reverse(*size));
    let mut out: Vec<String> = Vec::new();
    for url in found.into_iter().map(|(_, url)| url).chain(drawn) {
        if !out.contains(&url) {
            out.push(url);
        }
    }
    out
}

/// Asks the sites behind these links for their icons, on a thread of its own. `retry` asks again
/// a site that had none before, as when a link has just been saved. Tells `main` when any arrive.
pub fn fetch(dir: PathBuf, urls: &[String], retry: bool, main: SendHwnd, message: u32) {
    ENABLED.store(true, Ordering::SeqCst);
    let Ok(mut queue) = QUEUE.lock() else { return };
    for host in urls.iter().filter_map(|url| host_of(url)).filter(|host| is_public(host)) {
        if !queue.0.iter().any(|(queued, _)| *queued == host) {
            queue.0.push((host, retry));
        }
    }
    if queue.1 || queue.0.is_empty() {
        return;
    }
    queue.1 = true;
    drop(queue);
    let started = std::thread::Builder::new().name("orbit-link-icons".into()).stack_size(512 * 1024).spawn(move || {
        lower_priority();
        let fetched = run(&dir);
        if fetched {
            main.post(message, 0, 0);
        }
    });
    if started.is_err() {
        if let Ok(mut queue) = QUEUE.lock() {
            queue.1 = false;
        }
    }
}

/// Turned off: stop asking, and forget every icon and every answer.
pub fn forget(dir: &Path) {
    ENABLED.store(false, Ordering::SeqCst);
    if let Ok(mut queue) = QUEUE.lock() {
        queue.0.clear();
    }
    let folder = dir.join(FOLDER);
    let _ = std::thread::Builder::new().name("orbit-forget-icons".into()).spawn(move || {
        let _ = std::fs::remove_dir_all(folder);
    });
}

pub fn lower_priority() {
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_LOWEST};
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_LOWEST);
    }
}

/// Works through the queue. True if any icon was saved.
fn run(dir: &Path) -> bool {
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let mut saved = false;
    let session = Session::open();
    loop {
        let next = match QUEUE.lock() {
            Ok(mut queue) => match queue.0.pop() {
                Some(next) => next,
                None => {
                    queue.1 = false;
                    break;
                }
            },
            Err(_) => break,
        };
        if !ENABLED.load(Ordering::SeqCst) {
            continue;
        }
        let (host, retry) = next;
        let id = key(&host);
        if icon_file(dir, id).exists() || (!retry && known_to_have_none(dir, id)) {
            continue;
        }
        let icon = session.as_ref().and_then(|session| find_icon(session, &host));
        if !ENABLED.load(Ordering::SeqCst) {
            continue;
        }
        let _ = std::fs::create_dir_all(dir.join(FOLDER));
        let _ = std::fs::remove_file(tiled_icon_file(dir, id));
        match icon {
            Some(pixels) => {
                let _ = std::fs::remove_file(none_file(dir, id));
                saved |= crate::util::write_atomic(&icon_file(dir, id), &pixels).is_ok();
            }
            None => {
                let _ = std::fs::write(none_file(dir, id), NONE_VERSION);
            }
        }
    }
    drop(session);
    unsafe { CoUninitialize() };
    saved
}

/// The home page's own icons first, then the one every browser looks for.
fn find_icon(session: &Session, host: &str) -> Option<Vec<u8>> {
    let home = format!("https://{host}/");
    let mut candidates = Vec::new();
    if let Some((page, final_url)) = session.get(&home, host, PAGE_LIMIT, false) {
        let page_host = host_of(&final_url).unwrap_or_else(|| host.to_string());
        let scheme = if final_url.starts_with("http://") { "http" } else { "https" };
        candidates = icon_links(&String::from_utf8_lossy(&page), scheme, &page_host);
    }
    candidates.push(format!("https://{host}/favicon.ico"));
    for url in candidates.iter().take(4) {
        let Some(icon_host) = host_of(url) else { continue };
        if !same_site(host, &icon_host) || !is_public(&icon_host) {
            continue;
        }
        if let Some((bytes, _)) = session.get(url, host, ICON_LIMIT, true) {
            if let Some(pixels) = decode(&bytes) {
                return Some(pixels);
            }
        }
    }
    None
}

/// Whether a redirect from a page on `here` to `next` is followed, when asking `site`. Staying on the
/// link's own site is always fine. A request for an icon may also be sent, once, from the link's
/// own site to another public HTTPS address, because the site chose that itself. A page never
/// leaves its site, and nothing goes on from somewhere else to a third place.
fn may_follow(site: &str, here: &str, next: &str, icon: bool) -> bool {
    let Some(next_host) = host_of(next) else { return false };
    if !is_public(&next_host) {
        return false;
    }
    same_site(site, &next_host) || (icon && same_site(site, here) && next.starts_with("https://"))
}

/// The body of one HTTPS address, up to `limit` bytes, with the same care as an icon: Orbit's
/// name, no cookies, no sign-in, and redirects only within `site`. For the exchange rates.
pub fn get_from(url: &str, site: &str, limit: usize) -> Option<Vec<u8>> {
    Session::open()?.get(url, site, limit, false).map(|(body, _)| body)
}

/// One WinHTTP session: Orbit's name, the system's proxy settings, short timeouts.
struct Session(*mut core::ffi::c_void);

impl Session {
    fn open() -> Option<Session> {
        use windows::Win32::Networking::WinHttp::*;
        use windows::core::{PCWSTR, w};
        let handle = unsafe { WinHttpOpen(w!("Orbit"), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0) };
        if handle.is_null() {
            return None;
        }
        unsafe {
            let _ = WinHttpSetTimeouts(handle, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS);
        }
        Some(Session(handle))
    }

    /// The body of `url`, up to `limit` bytes, and the address it finally came from. Redirects are
    /// followed by hand, as [`may_follow`] allows. `icon` is whether this is a request for an icon.
    fn get(&self, url: &str, site: &str, limit: usize, icon: bool) -> Option<(Vec<u8>, String)> {
        let mut url = url.to_string();
        for _ in 0..=MOST_REDIRECTS {
            match self.request(&url, limit)? {
                Answer::Body(body) => return Some((body, url)),
                Answer::Redirect(location) => {
                    let scheme = if url.starts_with("http://") { "http" } else { "https" };
                    let here = host_of(&url)?;
                    let next = resolve(&location, scheme, &here)?;
                    if !may_follow(site, &here, &next, icon) {
                        return None;
                    }
                    url = next;
                }
            }
        }
        None
    }

    fn request(&self, url: &str, limit: usize) -> Option<Answer> {
        use windows::Win32::Networking::WinHttp::*;
        use windows::core::{PCWSTR, w};
        let secure = url.starts_with("https://");
        let rest = url.split_once("://")?.1;
        let (authority, path) = match rest.find(['/', '?']) {
            Some(at) => (&rest[..at], &rest[at..]),
            None => (rest, "/"),
        };
        let path = if path.starts_with('?') { format!("/{path}") } else { path.to_string() };
        let path = path.split('#').next().unwrap_or("/").to_string();
        let (name, port) = match authority.rsplit_once(':') {
            Some((name, port)) => (name, port.parse().ok()?),
            None => (authority, if secure { 443 } else { 80 }),
        };
        let name_w = crate::util::wide(name);
        let path_w = crate::util::wide(&path);
        let connection = Handle(unsafe { WinHttpConnect(self.0, PCWSTR(name_w.as_ptr()), port, 0) });
        if connection.0.is_null() {
            return None;
        }
        let flags = if secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
        let request = Handle(unsafe { WinHttpOpenRequest(connection.0, w!("GET"), PCWSTR(path_w.as_ptr()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), flags) });
        if request.0.is_null() {
            return None;
        }
        let option = |option: u32, value: u32| unsafe {
            let _ = WinHttpSetOption(Some(request.0), option, Some(&value.to_le_bytes()));
        };
        option(WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_DISABLE_COOKIES);
        option(WINHTTP_OPTION_AUTOLOGON_POLICY, WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH);
        option(WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER);
        unsafe {
            WinHttpSendRequest(request.0, None, None, 0, 0, 0).ok()?;
            WinHttpReceiveResponse(request.0, std::ptr::null_mut()).ok()?;
        }
        let status = query_number(request.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER)?;
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            return query_text(request.0, WINHTTP_QUERY_LOCATION).map(Answer::Redirect);
        }
        if status != 200 {
            return None;
        }
        let mut body = Vec::new();
        let mut chunk = vec![0u8; 16 * 1024];
        loop {
            let mut read = 0u32;
            unsafe { WinHttpReadData(request.0, chunk.as_mut_ptr() as *mut _, chunk.len() as u32, &mut read).ok()? };
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read as usize]);
            if body.len() >= limit {
                body.truncate(limit);
                break;
            }
        }
        Some(Answer::Body(body))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Networking::WinHttp::WinHttpCloseHandle(self.0);
        }
    }
}

enum Answer {
    Body(Vec<u8>),
    Redirect(String),
}

struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = windows::Win32::Networking::WinHttp::WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn query_number(request: *mut core::ffi::c_void, level: u32) -> Option<u32> {
    use windows::Win32::Networking::WinHttp::WinHttpQueryHeaders;
    let mut value = 0u32;
    let mut size = 4u32;
    let mut index = 0u32;
    unsafe { WinHttpQueryHeaders(request, level, windows::core::PCWSTR::null(), Some(&mut value as *mut u32 as *mut _), &mut size, &mut index).ok()? };
    Some(value)
}

fn query_text(request: *mut core::ffi::c_void, level: u32) -> Option<String> {
    use windows::Win32::Networking::WinHttp::WinHttpQueryHeaders;
    let mut buffer = vec![0u16; 2048];
    let mut size = (buffer.len() * 2) as u32;
    let mut index = 0u32;
    unsafe { WinHttpQueryHeaders(request, level, windows::core::PCWSTR::null(), Some(buffer.as_mut_ptr() as *mut _), &mut size, &mut index).ok()? };
    Some(String::from_utf16_lossy(&buffer[..size as usize / 2]))
}

/// An icon file as Orbit draws it: `ICON_SIZE` square, premultiplied BGRA. The logo fills the square
/// edge to edge, with any empty margin around it cut away and the corners rounded like app icons.
pub fn decode(bytes: &[u8]) -> Option<Vec<u8>> {
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    if is_svg(bytes) {
        return decode_svg(bytes);
    }
    unsafe {
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let stream = factory.CreateStream().ok()?;
        stream.InitializeFromMemory(bytes).ok()?;
        let decoder = factory.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
        // An icon file holds several sizes. The biggest that isn't huge looks best scaled down.
        let mut best: Option<(u32, IWICBitmapFrameDecode)> = None;
        for index in 0..decoder.GetFrameCount().ok()?.min(16) {
            let Ok(frame) = decoder.GetFrame(index) else { continue };
            let (mut width, mut height) = (0, 0);
            if frame.GetSize(&mut width, &mut height).is_err() || width == 0 || height == 0 {
                continue;
            }
            let side = width.max(height);
            let better = match &best {
                None => true,
                Some((current, _)) => (side <= LARGEST_FRAME && (side > *current || *current > LARGEST_FRAME)) || (*current > LARGEST_FRAME && side < *current),
            };
            if better {
                best = Some((side, frame));
            }
        }
        let (_, frame) = best?;
        let source = WICConvertBitmapSource(&GUID_WICPixelFormat32bppPBGRA, &frame).ok()?;
        let (mut width, mut height) = (0u32, 0u32);
        source.GetSize(&mut width, &mut height).ok()?;
        // Big enough to keep its detail once the margin is cut away, and never enlarged here.
        let (w, h, pixels) = scaled(&factory, &source, width, height, WORK_SIZE.min(width.max(height)))?;
        fill_square(&factory, &pixels, w, h)
    }
}

/// How large a picture is handled before it's made into an icon.
const WORK_SIZE: u32 = 256;

/// The logo without the empty margin around it, scaled to fill the icon's square and keeping its
/// shape. None if nothing in it can be seen.
fn fill_square(factory: &windows::Win32::Graphics::Imaging::IWICImagingFactory, pixels: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    use windows::Win32::Graphics::Imaging::{GUID_WICPixelFormat32bppPBGRA, WICBitmapInterpolationModeFant};
    let (w, h) = (width as usize, height as usize);
    let seen = |x: usize, y: usize| pixels[(y * w + x) * 4 + 3] > 16;
    let rows: Vec<usize> = (0..h).filter(|&y| (0..w).any(|x| seen(x, y))).collect();
    let columns: Vec<usize> = (0..w).filter(|&x| (0..h).any(|y| seen(x, y))).collect();
    let (top, bottom) = (*rows.first()?, *rows.last()?);
    let (left, right) = (*columns.first()?, *columns.last()?);
    let (crop_w, crop_h) = (right - left + 1, bottom - top + 1);
    let mut crop = Vec::with_capacity(crop_w * crop_h * 4);
    for y in top..=bottom {
        crop.extend_from_slice(&pixels[(y * w + left) * 4..(y * w + right + 1) * 4]);
    }
    let full = ICON_SIZE;
    let longest = crop_w.max(crop_h);
    let (fit_w, fit_h) = ((crop_w * full / longest).max(1), (crop_h * full / longest).max(1));
    let fitted = unsafe {
        let bitmap = factory.CreateBitmapFromMemory(crop_w as u32, crop_h as u32, &GUID_WICPixelFormat32bppPBGRA, (crop_w * 4) as u32, &crop).ok()?;
        let scaler = factory.CreateBitmapScaler().ok()?;
        scaler.Initialize(&bitmap, fit_w as u32, fit_h as u32, WICBitmapInterpolationModeFant).ok()?;
        let mut out = vec![0u8; fit_w * fit_h * 4];
        scaler.CopyPixels(std::ptr::null(), (fit_w * 4) as u32, &mut out).ok()?;
        out
    };
    Some(compose(&fitted, fit_w, fit_h))
}

/// An SVG file, whatever the server called it: its first tag, past any XML declaration, comments
/// and doctype, is `<svg`.
fn is_svg(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).to_ascii_lowercase();
    let head = head.trim_start_matches('\u{feff}').trim_start();
    head.starts_with("<svg") || ((head.starts_with("<?xml") || head.starts_with("<!--") || head.starts_with("<!doctype")) && head.contains("<svg"))
}

/// An SVG icon, drawn by Direct2D, which is part of Windows. It reads no file and no address that
/// the picture names. Like any other icon, it fills its square.
fn decode_svg(bytes: &[u8]) -> Option<Vec<u8>> {
    use windows::Win32::Graphics::Imaging::{CLSID_WICImagingFactory, IWICImagingFactory};
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    let pixels = render_svg(bytes, WORK_SIZE)?;
    let factory: IWICImagingFactory = unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }.ok()?;
    fill_square(&factory, &pixels, WORK_SIZE, WORK_SIZE)
}

/// The picture drawn into a `size` square, as premultiplied BGRA.
fn render_svg(bytes: &[u8], size: u32) -> Option<Vec<u8>> {
    use windows::Win32::Graphics::Direct2D::Common::{D2D_SIZE_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT};
    use windows::Win32::Graphics::Direct2D::*;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::core::Interface;
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let bitmap = wic.CreateBitmap(size, size, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad).ok()?;
        let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
        let properties = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        };
        let target = factory.CreateWicBitmapRenderTarget(&bitmap, &properties).ok()?;
        // Windows 10's Creators Update and later draw SVG. Before that, there's no icon.
        let context: ID2D1DeviceContext5 = target.cast().ok()?;
        let stream = windows::Win32::UI::Shell::SHCreateMemStream(Some(bytes))?;
        let document = context.CreateSvgDocument(&stream, D2D_SIZE_F { width: size as f32, height: size as f32 }).ok()?;
        target.BeginDraw();
        target.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        context.DrawSvgDocument(&document);
        target.EndDraw(None, None).ok()?;
        let mut pixels = vec![0u8; (size * size * 4) as usize];
        bitmap.CopyPixels(std::ptr::null(), size * 4, &mut pixels).ok()?;
        // A picture that drew nothing at all isn't an icon.
        pixels.chunks_exact(4).any(|pixel| pixel[3] > 0).then_some(pixels)
    }
}

/// The picture scaled to fit a `fit` square, keeping its shape.
unsafe fn scaled(factory: &windows::Win32::Graphics::Imaging::IWICImagingFactory, source: &windows::Win32::Graphics::Imaging::IWICBitmapSource, width: u32, height: u32, fit: u32) -> Option<(u32, u32, Vec<u8>)> {
    use windows::Win32::Graphics::Imaging::WICBitmapInterpolationModeFant;
    let longest = width.max(height).max(1);
    let (w, h) = ((width * fit / longest).max(1), (height * fit / longest).max(1));
    let scaler = unsafe { factory.CreateBitmapScaler() }.ok()?;
    unsafe { scaler.Initialize(source, w, h, WICBitmapInterpolationModeFant) }.ok()?;
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    unsafe { scaler.CopyPixels(std::ptr::null(), w * 4, &mut pixels) }.ok()?;
    Some((w, h, pixels))
}

/// Centres the picture in the square and rounds the corners.
fn compose(pixels: &[u8], width: usize, height: usize) -> Vec<u8> {
    let size = ICON_SIZE;
    let mut out = vec![0u8; ICON_BYTES];
    let (left, top) = ((size - width.min(size)) / 2, (size - height.min(size)) / 2);
    for y in 0..height.min(size) {
        for x in 0..width.min(size) {
            let from = (y * width + x) * 4;
            let to = ((top + y) * size + left + x) * 4;
            let alpha = pixels[from + 3] as u32;
            for channel in 0..4 {
                let under = out[to + channel] as u32;
                out[to + channel] = (pixels[from + channel] as u32 + under * (255 - alpha) / 255).min(255) as u8;
            }
        }
    }
    // Rounded like the app icons beside it.
    let radius = size as f32 * 0.24;
    for y in 0..size {
        for x in 0..size {
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            let dx = (radius - cx).max(cx - (size as f32 - radius));
            let dy = (radius - cy).max(cy - (size as f32 - radius));
            if dx <= 0.0 || dy <= 0.0 {
                continue;
            }
            let coverage = (radius - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            let at = (y * size + x) * 4;
            for channel in 0..4 {
                out[at + channel] = (out[at + channel] as f32 * coverage + 0.5) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_host_of_a_web_address_is_used() {
        assert_eq!(host_of("https://GitHub.com/rust-lang/rust?x=1").as_deref(), Some("github.com"));
        assert_eq!(host_of("http://user:pw@example.org:8080/a").as_deref(), Some("example.org:8080"));
        assert_eq!(host_of("mailto:a@b.c"), None);
        assert_eq!(host_of("spotify:track:1"), None);
        assert_eq!(host_of("https://{team}.slack.com/"), None, "a blank in the host can't be asked about");
    }

    #[test]
    fn private_and_reserved_names_are_never_asked() {
        for host in ["github.com", "docs.rs", "8.8.8.8", "bbc.co.uk:443"] {
            assert!(is_public(host), "{host}");
        }
        for host in ["localhost", "intranet", "nas.local", "printer.lan", "10.0.0.5", "192.168.1.1", "172.20.0.1", "127.0.0.1:3000", "169.254.1.1", "[::1]", "example.invalid", "a.test", "x.example", "router.home.arpa", "app.internal"] {
            assert!(!is_public(host), "{host}");
        }
    }

    #[test]
    fn icons_from_another_site_are_refused() {
        assert!(same_site("github.com", "github.com"));
        assert!(same_site("github.com", "www.github.com"));
        assert!(same_site("www.github.com", "github.com"));
        assert!(!same_site("github.com", "githubassets.com"));
        assert!(!same_site("github.com", "evilgithub.com"));
        assert!(!same_site("example.org", "icons.duckduckgo.com"));
    }

    #[test]
    fn a_page_names_its_icons_best_first() {
        let html = r#"<html><head>
            <link rel="stylesheet" href="/a.css">
            <LINK REL="icon" type="image/png" sizes="32x32" href="/icon-32.png">
            <link rel='shortcut icon' href='favicon.ico'>
            <link rel="apple-touch-icon" href="//cdn.example.org/touch.png">
            <link rel="icon" type="image/svg+xml" href="/icon.svg">
            <link rel=icon sizes=192x192 href=/big.png>
            <link rel="icon" href="data:image/png;base64,AAAA">
        </head>"#;
        let found = icon_links(html, "https", "example.org");
        assert_eq!(
            found,
            ["https://example.org/big.png", "https://cdn.example.org/touch.png", "https://example.org/icon-32.png", "https://example.org/favicon.ico", "https://example.org/icon.svg"],
            "SVG comes after every picture"
        );
    }

    #[test]
    fn an_svg_logo_fills_its_square() {
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let full = br##"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="#0000ff"/></svg>"##;
        let pixels = decode(full).expect("Direct2D draws SVG");
        let at = |pixels: &[u8], x: usize, y: usize| pixels[(y * ICON_SIZE + x) * 4..(y * ICON_SIZE + x) * 4 + 4].to_vec();
        assert_eq!(at(&pixels, 32, 32), [255, 0, 0, 255], "blue, filling its square");
        let dot = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><circle cx="5" cy="5" r="2" fill="#000"/></svg>"##;
        let pixels = decode(dot).expect("Direct2D draws SVG");
        assert_eq!(at(&pixels, 32, 32), [0, 0, 0, 255], "the dot in the middle");
        assert_eq!(at(&pixels, 32, 3)[3], 255, "grown to reach the top, with no margin or tile");
        assert_eq!(at(&pixels, 4, 4)[3], 0, "and nothing behind it");
        assert_eq!(decode(br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"></svg>"#), None, "nothing drawn is no icon");
    }

    #[test]
    fn a_site_marked_before_svg_was_read_is_asked_again() {
        let dir = std::env::temp_dir().join(format!("orbit-none-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(FOLDER)).unwrap();
        std::fs::write(none_file(&dir, 7), b"").unwrap();
        assert!(!known_to_have_none(&dir, 7));
        std::fs::write(none_file(&dir, 7), NONE_VERSION).unwrap();
        assert!(known_to_have_none(&dir, 7));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A small 24-bit bitmap file of one colour, which Windows decodes like any picture.
    fn bmp(width: u32, height: u32, bgr: [u8; 3]) -> Vec<u8> {
        let row = (width * 3).div_ceil(4) * 4;
        let data = row * height;
        let mut out = Vec::new();
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(54 + data).to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&(width as i32).to_le_bytes());
        out.extend_from_slice(&(height as i32).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        out.extend_from_slice(&[0; 24]);
        for _ in 0..height {
            for _ in 0..width {
                out.extend_from_slice(&bgr);
            }
            out.resize(out.len() + (row - width * 3) as usize, 0);
        }
        out
    }

    #[test]
    fn a_picture_decodes_to_a_rounded_icon() {
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let pixels = decode(&bmp(2, 2, [0, 0, 255])).expect("Windows decodes a bitmap");
        assert_eq!(pixels.len(), ICON_BYTES);
        let at = |x: usize, y: usize| &pixels[(y * ICON_SIZE + x) * 4..(y * ICON_SIZE + x) * 4 + 4];
        assert_eq!(at(32, 32), [0, 0, 255, 255], "red, filling its square");
        assert_eq!(at(0, 0)[3], 0, "with the corners rounded off");
        assert_eq!(decode(b"<html>not a picture</html>"), None);
    }

    /// A one-answer web server on this computer, which hands back what it was asked.
    fn serve_once(answer: &'static str) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut asked = Vec::new();
            let mut chunk = [0u8; 1024];
            while !asked.ends_with(b"\r\n\r\n") {
                let read = stream.read(&mut chunk).unwrap_or(0);
                if read == 0 {
                    break;
                }
                asked.extend_from_slice(&chunk[..read]);
            }
            stream.write_all(answer.as_bytes()).unwrap();
            String::from_utf8_lossy(&asked).to_string()
        });
        (address, handle)
    }

    #[test]
    fn a_request_names_orbit_and_carries_nothing_else() {
        let (address, server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello");
        let session = Session::open().unwrap();
        let Some(Answer::Body(body)) = session.request(&format!("{address}/favicon.ico"), 3) else { panic!("no answer") };
        assert_eq!(body, b"hel", "no more than the limit is read");
        let asked = server.join().unwrap();
        assert!(asked.starts_with("GET /favicon.ico HTTP/1.1\r\n"), "{asked}");
        assert!(asked.contains("User-Agent: Orbit\r\n"), "{asked}");
        assert!(!asked.to_ascii_lowercase().contains("cookie"), "{asked}");
        assert!(!asked.to_ascii_lowercase().contains("authorization"), "{asked}");
    }

    /// Asks real sites, so it only runs when asked for by name, with the sites in
    /// `ORBIT_ICON_HOSTS` and the folder in `ORBIT_ICON_DIR`.
    #[test]
    #[ignore]
    fn live_sites() {
        let (Ok(hosts), Ok(dir)) = (std::env::var("ORBIT_ICON_HOSTS"), std::env::var("ORBIT_ICON_DIR")) else { return };
        let dir = PathBuf::from(dir);
        let urls: Vec<String> = hosts.split(',').map(|host| format!("https://{host}/")).collect();
        if let Ok(mut queue) = QUEUE.lock() {
            queue.0 = urls.iter().filter_map(|url| host_of(url)).map(|host| (host, true)).collect();
            queue.1 = true;
        }
        run(&dir);
        for url in &urls {
            let host = host_of(url).unwrap();
            println!("{host}: {}", if icon_file(&dir, key(&host)).exists() { "icon" } else { "none" });
        }
    }

    #[test]
    fn an_icon_follows_its_own_sites_redirect_once() {
        let gmail = "mail.google.com";
        assert!(may_follow(gmail, gmail, "https://ssl.gstatic.com/ui/v1/icons/mail/favicon.ico", true), "the site's own choice");
        assert!(!may_follow(gmail, gmail, "https://ssl.gstatic.com/favicon.ico", false), "but a page stays on its site");
        assert!(!may_follow(gmail, "ssl.gstatic.com", "https://third.example.org/x.ico", true), "and goes no further");
        assert!(!may_follow(gmail, gmail, "http://ssl.gstatic.com/favicon.ico", true), "only over HTTPS");
        assert!(!may_follow(gmail, gmail, "https://192.168.1.1/favicon.ico", true), "never somewhere private");
        assert!(may_follow(gmail, "ssl.gstatic.com", "https://mail.google.com/x.ico", false), "back to the site is fine");
    }

    #[test]
    fn a_redirect_to_another_site_is_not_followed() {
        let (address, server) = serve_once("HTTP/1.1 302 Found\r\nLocation: https://icons.elsewhere.example.org/x.png\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let session = Session::open().unwrap();
        let site = host_of(&address).unwrap();
        assert_eq!(session.get(&format!("{address}/"), &site, 100, false), None);
        server.join().unwrap();
    }
}
