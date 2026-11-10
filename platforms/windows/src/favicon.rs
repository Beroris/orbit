//! Quicklink icons, asked of each link's own site. One switch in Settings turns them off.
//!
//! When Orbit starts, and when the launcher opens a while later, every link with no icon yet has its
//! site asked; saving a link asks its site again even if it has one. Nothing is kept about a site
//! that gave none, so one that couldn't be reached, or had nothing then, is asked again later.
//! Requests carry no cookies and no Windows sign-in, wait at most a few seconds, and read a bounded
//! amount. Only the address's host is used: never its path or query, which may hold something
//! personal. The site may send Orbit on to another public HTTPS address, as notion.so sends it to
//! notion.com, and its page may name icons kept on one, such as its own file server, as a browser
//! would load them. Nothing goes to a private address unless it's the link's own site. Sites on your
//! own network are asked too, over HTTP when they have no HTTPS, and their certificates aren't
//! checked: nothing in the request needs one, and an icon is all that comes back.

use crate::indexer::{ICON_BYTES, ICON_SIZE};
use crate::util::SendHwnd;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

pub const FOLDER: &str = "link-icons";
/// Enough of a page to reach its `<head>`.
const PAGE_LIMIT: usize = 256 * 1024;
const ICON_LIMIT: usize = 1024 * 1024;
/// How long a request you wait for, such as the weather, may take.
const TIMEOUT_MS: i32 = 5000;
/// Icons are asked for in the background, and some sites take longer to answer.
const ICON_TIMEOUT_MS: i32 = 8000;
/// Some sites go round several addresses first, as sharepoint.com does to reach microsoft.com.
const MOST_REDIRECTS: usize = 8;
/// The largest picture decoded. A bigger frame in an icon file is passed over for a smaller one.
const LARGEST_FRAME: u32 = 1024;
/// Where sites usually keep an icon, for one whose page names none or can't be read.
const USUAL_PLACES: [&str; 4] = ["/apple-touch-icon.png", "/favicon.ico", "/favicon.svg", "/favicon.png"];
/// The most icon addresses tried for one site.
const MOST_TRIES: usize = 8;
/// Names Orbit, in the form sites expect from a program that isn't a browser. Some turn away a
/// request that doesn't start with `Mozilla/5.0`.
const ICON_AGENT: &str = "Mozilla/5.0 (compatible; Orbit)";

/// Off in Settings: anything queued or under way stops before it writes.
static ENABLED: AtomicBool = AtomicBool::new(true);
/// Sites waiting to be asked, and whether a thread is already asking.
static QUEUE: Mutex<(Vec<Ask>, bool)> = Mutex::new((Vec::new(), false));

/// A site to ask for its icon.
struct Ask {
    host: String,
    /// Asked even if an icon is kept, as when its link has just been saved.
    again: bool,
    /// May be asked over plain HTTP: a site on your own network, or a link given as `http://`.
    plain: bool,
}

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

/// A name on the public internet, rather than one on your own network: a local name, a private
/// address, a name reserved for testing or a bare IPv6 address. A site may send Orbit on to a public
/// address only, so a site on the internet can't point it at something on your network.
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

/// The key of a link's icon, if it's a web address Orbit can ask.
pub fn link_key(url: &str) -> Option<u64> {
    host_of(url).map(|host| key(&host))
}

/// Named for how it's drawn, so icons kept from before logos filled their square are fetched again.
pub fn icon_file(dir: &Path, key: u64) -> PathBuf {
    dir.join(FOLDER).join(format!("{key:016x}-full.bgra"))
}

/// Where an icon drawn on a white tile, as earlier versions did, was kept.
fn tiled_icon_file(dir: &Path, key: u64) -> PathBuf {
    dir.join(FOLDER).join(format!("{key:016x}.bgra"))
}

/// Where an earlier version noted that a site had no icon, which kept it from being asked again.
/// Such a note is removed when the site is next asked.
fn none_file(dir: &Path, key: u64) -> PathBuf {
    dir.join(FOLDER).join(format!("{key:016x}.none"))
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

/// Asks the sites behind these links for their icons, on a thread of its own: those with no icon
/// kept, or, with `again`, every one, as when a link has just been saved. A site that gives none
/// keeps the icon it had. Tells `main` when any arrive.
pub fn fetch(dir: PathBuf, urls: &[String], again: bool, main: SendHwnd, message: u32) {
    ENABLED.store(true, Ordering::SeqCst);
    let Ok(mut queue) = QUEUE.lock() else { return };
    for url in urls {
        let Some(host) = host_of(url) else { continue };
        let plain = url.trim().to_ascii_lowercase().starts_with("http://") || !is_public(&host);
        match queue.0.iter_mut().find(|ask| ask.host == host) {
            Some(ask) => {
                ask.again |= again;
                ask.plain |= plain;
            }
            None => queue.0.push(Ask { host, again, plain }),
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
    let session = Session::open(ICON_AGENT, ICON_TIMEOUT_MS);
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
        let id = key(&next.host);
        let _ = std::fs::remove_file(none_file(dir, id));
        if !next.again && icon_file(dir, id).exists() {
            continue;
        }
        let icon = session.as_ref().and_then(|session| find_icon(session, &next.host, next.plain));
        if !ENABLED.load(Ordering::SeqCst) {
            continue;
        }
        if let Some(pixels) = icon {
            let _ = std::fs::create_dir_all(dir.join(FOLDER));
            let _ = std::fs::remove_file(tiled_icon_file(dir, id));
            saved |= crate::util::write_atomic(&icon_file(dir, id), &pixels).is_ok();
        }
    }
    drop(session);
    unsafe { CoUninitialize() };
    saved
}

/// The site's icon: the best its home page names, then the usual places. `plain` lets it be asked
/// over HTTP when it doesn't answer over HTTPS.
fn find_icon(session: &Session, host: &str, plain: bool) -> Option<Vec<u8>> {
    let mut scheme = "https";
    let mut page = session.get(&format!("https://{host}/"), host, PAGE_LIMIT, true);
    if plain && matches!(page, Err(Miss::Unreachable)) {
        scheme = "http";
        page = session.get(&format!("http://{host}/"), host, PAGE_LIMIT, true);
    }
    if matches!(page, Err(Miss::Unreachable)) {
        return None; // nothing else there will answer either
    }
    let (named, moved) = match &page {
        Ok((body, final_url)) => {
            let there = host_of(final_url).unwrap_or_else(|| host.to_string());
            let page_scheme = if final_url.starts_with("http://") { "http" } else { "https" };
            (icon_links(&String::from_utf8_lossy(body), page_scheme, &there), !same_site(host, &there))
        }
        Err(_) => (Vec::new(), false),
    };
    let usual = USUAL_PLACES.iter().map(|path| format!("{scheme}://{host}{path}"));
    // A page that sent Orbit to another site, such as a sign-in page, may show that site's icon,
    // so the usual places on the link's own site come first then.
    let candidates: Vec<String> = if moved { usual.chain(named).collect() } else { named.into_iter().chain(usual).collect() };
    let mut tried: Vec<&str> = Vec::new();
    // Another site that turned Orbit away, or didn't have two of the icons the page named, isn't
    // asked for the rest, which leaves tries for the link's own site. Such icons are often kept
    // for the site's own pages, or long gone.
    let mut misses: Vec<String> = Vec::new();
    for url in &candidates {
        if tried.len() == MOST_TRIES {
            break;
        }
        let there = host_of(url).unwrap_or_default();
        if tried.contains(&url.as_str()) || misses.iter().filter(|miss| **miss == there).count() >= 2 || !may_follow(host, url, true) {
            continue;
        }
        tried.push(url);
        let got = session.get(url, host, ICON_LIMIT, true);
        let turned_away = matches!(got, Err(Miss::Status(401 | 403 | 429) | Miss::Unreachable));
        if let Some(pixels) = got.ok().and_then(|(bytes, _)| decode(&bytes)) {
            return Some(pixels);
        }
        if !same_site(host, &there) {
            misses.extend(std::iter::repeat_n(there, if turned_away { 2 } else { 1 }));
        }
    }
    None
}

/// Whether Orbit may go to `next`, named or sent there while asking `site`. Anything on the link's
/// own site is fine. With `roam`, so is a public HTTPS address elsewhere, as a browser would go: the
/// site chose it. Never a private address that isn't the site's own, so a site on the internet can't
/// point Orbit at something on your network.
fn may_follow(site: &str, next: &str, roam: bool) -> bool {
    let Some(there) = host_of(next) else { return false };
    if same_site(site, &there) {
        return roam || is_public(&there);
    }
    roam && next.trim().to_ascii_lowercase().starts_with("https://") && is_public(&there)
}

/// Why nothing came back.
#[derive(Debug, PartialEq)]
enum Miss {
    /// The server answered, with this status.
    Status(u32),
    /// The address asked didn't answer, or not in time.
    Unreachable,
    /// It sent Orbit somewhere it doesn't go, round too many times, or somewhere that didn't answer.
    Refused,
}

/// The body of one HTTPS address, up to `limit` bytes, with the same care as an icon: Orbit's
/// name, no cookies, no sign-in, and redirects only within `site`. For the exchange rates.
pub fn get_from(url: &str, site: &str, limit: usize) -> Option<Vec<u8>> {
    Session::open("Orbit", TIMEOUT_MS)?.get(url, site, limit, false).ok().map(|(body, _)| body)
}

/// Why a signed-in request gave nothing.
#[derive(Debug, PartialEq)]
pub enum Failure {
    /// The server answered with this status, such as 401 for a sign-in it won't take.
    Status(u32),
    /// It couldn't be reached, or the answer didn't arrive.
    Unreachable,
}

/// The body of one HTTPS address asked for with these extra headers, such as a sign-in, up to
/// `limit` bytes. No cookies, and no redirects at all, so a sign-in can't be passed on elsewhere.
pub fn get_signed_in(url: &str, headers: &[(&str, &str)], limit: usize) -> Result<Vec<u8>, Failure> {
    use windows::Win32::Networking::WinHttp::*;
    use windows::core::{PCWSTR, w};
    let session = Session::open("Orbit", TIMEOUT_MS).ok_or(Failure::Unreachable)?;
    let rest = url.strip_prefix("https://").ok_or(Failure::Unreachable)?;
    let (name, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    };
    let name_w = crate::util::wide(name);
    let path_w = crate::util::wide(path);
    let connection = Handle(unsafe { WinHttpConnect(session.0, PCWSTR(name_w.as_ptr()), 443, 0) });
    if connection.0.is_null() {
        return Err(Failure::Unreachable);
    }
    let request = Handle(unsafe { WinHttpOpenRequest(connection.0, w!("GET"), PCWSTR(path_w.as_ptr()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE) });
    if request.0.is_null() {
        return Err(Failure::Unreachable);
    }
    let option = |option: u32, value: u32| unsafe {
        let _ = WinHttpSetOption(Some(request.0), option, Some(&value.to_le_bytes()));
    };
    option(WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_DISABLE_COOKIES);
    option(WINHTTP_OPTION_AUTOLOGON_POLICY, WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH);
    option(WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER);
    let mut lines = String::new();
    for (header, value) in headers {
        // A header can't carry another one inside it.
        if header.contains(['\r', '\n', ':']) || value.contains(['\r', '\n']) {
            return Err(Failure::Unreachable);
        }
        lines.push_str(&format!("{header}: {value}\r\n"));
    }
    let lines_w: Vec<u16> = lines.encode_utf16().collect();
    unsafe {
        if !lines_w.is_empty() {
            WinHttpAddRequestHeaders(request.0, &lines_w, WINHTTP_ADDREQ_FLAG_ADD | WINHTTP_ADDREQ_FLAG_REPLACE).map_err(|_| Failure::Unreachable)?;
        }
        WinHttpSendRequest(request.0, None, None, 0, 0, 0).map_err(|_| Failure::Unreachable)?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|_| Failure::Unreachable)?;
    }
    let status = query_number(request.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER).ok_or(Failure::Unreachable)?;
    if status != 200 {
        return Err(Failure::Status(status));
    }
    let mut body = Vec::new();
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        let mut read = 0u32;
        unsafe { WinHttpReadData(request.0, chunk.as_mut_ptr() as *mut _, chunk.len() as u32, &mut read).map_err(|_| Failure::Unreachable)? };
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read as usize]);
        if body.len() >= limit {
            body.truncate(limit);
            break;
        }
    }
    Ok(body)
}

/// One WinHTTP session: a user agent naming Orbit, the system's proxy settings, short timeouts.
struct Session(*mut core::ffi::c_void);

impl Session {
    fn open(agent: &str, timeout_ms: i32) -> Option<Session> {
        use windows::Win32::Networking::WinHttp::*;
        use windows::core::PCWSTR;
        let agent = crate::util::wide(agent);
        let handle = unsafe { WinHttpOpen(PCWSTR(agent.as_ptr()), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0) };
        if handle.is_null() {
            return None;
        }
        unsafe {
            let _ = WinHttpSetTimeouts(handle, timeout_ms, timeout_ms, timeout_ms, timeout_ms);
        }
        Some(Session(handle))
    }

    /// The body of `url`, up to `limit` bytes, and the address it finally came from. Redirects are
    /// followed by hand, as [`may_follow`] allows, with `roam` as it says.
    fn get(&self, url: &str, site: &str, limit: usize, roam: bool) -> Result<(Vec<u8>, String), Miss> {
        let mut url = url.to_string();
        for hop in 0..=MOST_REDIRECTS {
            let answer = match self.request(&url, limit) {
                // The site answered, so it's there, even if where it sent Orbit isn't.
                Err(Miss::Unreachable) if hop > 0 => return Err(Miss::Refused),
                answer => answer?,
            };
            match answer {
                Answer::Body(body) => return Ok((body, url)),
                Answer::Redirect(location) => {
                    let scheme = if url.starts_with("http://") { "http" } else { "https" };
                    let here = host_of(&url).ok_or(Miss::Refused)?;
                    let next = resolve(&location, scheme, &here).ok_or(Miss::Refused)?;
                    if !may_follow(site, &next, roam) {
                        return Err(Miss::Refused);
                    }
                    url = next;
                }
            }
        }
        Err(Miss::Refused)
    }

    fn request(&self, url: &str, limit: usize) -> Result<Answer, Miss> {
        use windows::Win32::Networking::WinHttp::*;
        use windows::core::{PCWSTR, w};
        let secure = url.starts_with("https://");
        let rest = url.split_once("://").ok_or(Miss::Refused)?.1;
        let (authority, path) = match rest.find(['/', '?']) {
            Some(at) => (&rest[..at], &rest[at..]),
            None => (rest, "/"),
        };
        let path = if path.starts_with('?') { format!("/{path}") } else { path.to_string() };
        let path = path.split('#').next().unwrap_or("/").to_string();
        let (name, port) = match authority.rsplit_once(':') {
            Some((name, port)) => (name, port.parse().map_err(|_| Miss::Refused)?),
            None => (authority, if secure { 443 } else { 80 }),
        };
        let name_w = crate::util::wide(name);
        let path_w = crate::util::wide(&path);
        let connection = Handle(unsafe { WinHttpConnect(self.0, PCWSTR(name_w.as_ptr()), port, 0) });
        if connection.0.is_null() {
            return Err(Miss::Unreachable);
        }
        let flags = if secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
        let request = Handle(unsafe { WinHttpOpenRequest(connection.0, w!("GET"), PCWSTR(path_w.as_ptr()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), flags) });
        if request.0.is_null() {
            return Err(Miss::Unreachable);
        }
        let option = |option: u32, value: u32| unsafe {
            let _ = WinHttpSetOption(Some(request.0), option, Some(&value.to_le_bytes()));
        };
        option(WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_DISABLE_COOKIES);
        option(WINHTTP_OPTION_AUTOLOGON_POLICY, WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH);
        option(WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER);
        if secure && !is_public(name) {
            // A site on your own network often has a certificate of its own making. Nothing in the
            // request needs one: it carries no cookies and no sign-in, and an icon is all that
            // comes back.
            option(WINHTTP_OPTION_SECURITY_FLAGS, SECURITY_FLAG_IGNORE_UNKNOWN_CA | SECURITY_FLAG_IGNORE_CERT_CN_INVALID | SECURITY_FLAG_IGNORE_CERT_DATE_INVALID | SECURITY_FLAG_IGNORE_CERT_WRONG_USAGE);
        }
        unsafe {
            WinHttpSendRequest(request.0, None, None, 0, 0, 0).map_err(|_| Miss::Unreachable)?;
            WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|_| Miss::Unreachable)?;
        }
        let status = query_number(request.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER).ok_or(Miss::Unreachable)?;
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            return query_text(request.0, WINHTTP_QUERY_LOCATION).map(Answer::Redirect).ok_or(Miss::Status(status));
        }
        if status != 200 {
            return Err(Miss::Status(status));
        }
        let mut body = Vec::new();
        let mut chunk = vec![0u8; 16 * 1024];
        loop {
            let mut read = 0u32;
            unsafe { WinHttpReadData(request.0, chunk.as_mut_ptr() as *mut _, chunk.len() as u32, &mut read).map_err(|_| Miss::Unreachable)? };
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read as usize]);
            if body.len() >= limit {
                body.truncate(limit);
                break;
            }
        }
        Ok(Answer::Body(body))
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
    if is_svg(bytes) {
        return decode_svg(bytes);
    }
    // Windows can't read some icon files whole, such as one whose PNG uses a palette, though the
    // PNGs inside read fine on their own.
    decode_picture(bytes).or_else(|| embedded_pngs(bytes).into_iter().find_map(decode_picture))
}

/// The PNG pictures inside an `.ico` file, biggest first. Empty for anything else.
fn embedded_pngs(bytes: &[u8]) -> Vec<&[u8]> {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    let word = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let long = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    if word(0) != Some(0) || word(2) != Some(1) {
        return Vec::new();
    }
    let count = word(4).unwrap_or(0).min(64);
    let mut found: Vec<(usize, &[u8])> = (0..count)
        .filter_map(|i| {
            let entry = 6 + i * 16;
            let width = match *bytes.get(entry)? {
                0 => 256,
                w => w as usize,
            };
            let (size, offset) = (long(entry + 8)?, long(entry + 12)?);
            let picture = bytes.get(offset..offset.checked_add(size)?)?;
            picture.starts_with(PNG).then_some((width, picture))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, picture)| picture).collect()
}

/// A picture Windows Imaging Component reads (PNG, ICO, JPEG, GIF, BMP, WebP), made into an icon.
fn decode_picture(bytes: &[u8]) -> Option<Vec<u8>> {
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
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
    fn names_on_your_own_network_are_told_apart() {
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
        let session = Session::open(ICON_AGENT, ICON_TIMEOUT_MS).unwrap();
        let Ok(Answer::Body(body)) = session.request(&format!("{address}/favicon.ico"), 3) else { panic!("no answer") };
        assert_eq!(body, b"hel", "no more than the limit is read");
        let asked = server.join().unwrap();
        assert!(asked.starts_with("GET /favicon.ico HTTP/1.1\r\n"), "{asked}");
        assert!(asked.contains("User-Agent: Mozilla/5.0 (compatible; Orbit)\r\n"), "{asked}");
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
            queue.0 = urls.iter().filter_map(|url| host_of(url)).map(|host| Ask { host, again: true, plain: false }).collect();
            queue.1 = true;
        }
        run(&dir);
        for url in &urls {
            let host = host_of(url).unwrap();
            println!("{host}: {}", if icon_file(&dir, key(&host)).exists() { "icon" } else { "none" });
        }
    }

    #[test]
    fn where_orbit_may_go() {
        let gmail = "mail.google.com";
        assert!(may_follow(gmail, "https://ssl.gstatic.com/ui/v1/icons/mail/favicon.ico", true), "a public HTTPS address the site chose");
        assert!(!may_follow(gmail, "https://ssl.gstatic.com/favicon.ico", false), "but not when asked to stay on the site");
        assert!(!may_follow(gmail, "http://ssl.gstatic.com/favicon.ico", true), "elsewhere only over HTTPS");
        assert!(!may_follow(gmail, "https://192.168.1.1/favicon.ico", true), "never somewhere private");
        assert!(!may_follow(gmail, "https://nas.local/favicon.ico", true), "by name or by address");
        assert!(may_follow("nas.local", "http://nas.local/favicon.ico", true), "a site on your network is its own, even over HTTP");
        assert!(may_follow("intranet", "https://intranet/logo.png", true), "and so is a bare name");
        assert!(!may_follow("nas.local", "https://router.local/favicon.ico", true), "but it can't send Orbit to another one");
        assert!(may_follow("nas.local", "https://cdn.example.org/logo.png", true), "only out to the internet");
    }

    #[test]
    fn a_redirect_somewhere_private_is_not_followed() {
        let (address, server) = serve_once("HTTP/1.1 302 Found\r\nLocation: https://192.168.1.1/x.png\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let session = Session::open(ICON_AGENT, ICON_TIMEOUT_MS).unwrap();
        let site = host_of(&address).unwrap();
        assert_eq!(session.get(&format!("{address}/"), &site, 100, true), Err(Miss::Refused));
        server.join().unwrap();
        let (address, server) = serve_once("HTTP/1.1 302 Found\r\nLocation: https://icons.elsewhere.example.org/x.png\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let site = host_of(&address).unwrap();
        assert_eq!(session.get(&format!("{address}/"), &site, 100, false), Err(Miss::Refused), "nor anywhere else, for a request that stays on its site");
        server.join().unwrap();
    }

    /// A web server on this computer that answers a page and its icon over HTTP, and turns away
    /// HTTPS, like a device on your network. Each request's first line is sent on the channel.
    fn serve_site(icon: Vec<u8>) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        let (asked, heard) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
                let mut request = Vec::new();
                let mut chunk = [0u8; 1024];
                while !request.ends_with(b"\r\n\r\n") {
                    let read = stream.read(&mut chunk).unwrap_or(0);
                    if read == 0 || (request.is_empty() && chunk[0] == 0x16) {
                        break; // gone, or a TLS greeting, which this server doesn't speak
                    }
                    request.extend_from_slice(&chunk[..read]);
                }
                let line = String::from_utf8_lossy(&request).lines().next().unwrap_or("").to_string();
                let (kind, body) = match line.split(' ').nth(1) {
                    Some("/") => ("text/html", br#"<html><head><link rel="icon" href="/logo.bmp"></head></html>"#.to_vec()),
                    Some("/logo.bmp") => ("image/bmp", icon.clone()),
                    Some(_) => ("text/plain", Vec::new()),
                    None => continue,
                };
                let status = if body.is_empty() { "404 Not Found" } else { "200 OK" };
                let head = format!("HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(&body));
                let _ = asked.send(line);
            }
        });
        (address, heard)
    }

    #[test]
    fn a_site_on_your_network_gives_its_icon_over_http() {
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let (host, asked) = serve_site(bmp(2, 2, [0, 255, 0]));
        let session = Session::open(ICON_AGENT, ICON_TIMEOUT_MS).unwrap();
        let pixels = find_icon(&session, &host, true).expect("the page's icon, over HTTP");
        assert_eq!(&pixels[(32 * ICON_SIZE + 32) * 4..(32 * ICON_SIZE + 32) * 4 + 4], [0, 255, 0, 255]);
        let asked: Vec<String> = asked.try_iter().collect();
        assert_eq!(asked, ["GET / HTTP/1.1", "GET /logo.bmp HTTP/1.1"]);
    }

    #[test]
    fn a_note_that_a_site_had_no_icon_is_dropped_and_the_site_asked() {
        let dir = std::env::temp_dir().join(format!("orbit-none-test-{}", std::process::id()));
        let (host, _asked) = serve_site(bmp(2, 2, [255, 0, 0]));
        let id = key(&host);
        std::fs::create_dir_all(dir.join(FOLDER)).unwrap();
        std::fs::write(none_file(&dir, id), b"2").unwrap();
        if let Ok(mut queue) = QUEUE.lock() {
            queue.0.push(Ask { host, again: false, plain: true });
            queue.1 = true;
        }
        assert!(run(&dir), "the icon was saved");
        assert!(!none_file(&dir, id).exists());
        assert_eq!(std::fs::read(icon_file(&dir, id)).unwrap().len(), ICON_BYTES);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A PNG file of one colour, drawn from a palette, which Windows reads on its own but not
    /// always inside an icon file.
    fn palette_png(side: u32, rgb: [u8; 3]) -> Vec<u8> {
        fn crc(bytes: &[u8]) -> u32 {
            let mut crc = !0u32;
            for &byte in bytes {
                crc ^= byte as u32;
                for _ in 0..8 {
                    crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
                }
            }
            !crc
        }
        fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let mut tagged = kind.to_vec();
            tagged.extend_from_slice(data);
            out.extend_from_slice(&tagged);
            out.extend_from_slice(&crc(&tagged).to_be_bytes());
        }
        let mut raw = Vec::new();
        for _ in 0..side {
            raw.push(0); // no filter
            raw.extend(std::iter::repeat_n(0u8, side as usize));
        }
        let (mut a, mut b) = (1u32, 0u32);
        for &byte in &raw {
            a = (a + byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        let mut zlib = vec![0x78, 0x01, 0x01];
        zlib.extend_from_slice(&(raw.len() as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
        zlib.extend_from_slice(&raw);
        zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
        let mut header = Vec::new();
        header.extend_from_slice(&side.to_be_bytes());
        header.extend_from_slice(&side.to_be_bytes());
        header.extend_from_slice(&[8, 3, 0, 0, 0]);
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        chunk(&mut out, b"IHDR", &header);
        chunk(&mut out, b"PLTE", &rgb);
        chunk(&mut out, b"IDAT", &zlib);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    /// An icon file holding these PNGs, each with its side.
    fn ico(pngs: &[(u8, &[u8])]) -> Vec<u8> {
        let mut out = vec![0, 0, 1, 0];
        out.extend_from_slice(&(pngs.len() as u16).to_le_bytes());
        let mut offset = 6 + 16 * pngs.len();
        for (side, png) in pngs {
            out.extend_from_slice(&[*side, *side, 0, 0, 1, 0, 32, 0]);
            out.extend_from_slice(&(png.len() as u32).to_le_bytes());
            out.extend_from_slice(&(offset as u32).to_le_bytes());
            offset += png.len();
        }
        for (_, png) in pngs {
            out.extend_from_slice(png);
        }
        out
    }

    #[test]
    fn an_icon_file_windows_cant_read_whole_gives_up_its_pngs() {
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let (small, big) = (palette_png(16, [0, 0, 255]), palette_png(48, [255, 0, 0]));
        let file = ico(&[(16, &small), (48, &big)]);
        assert_eq!(embedded_pngs(&file), [&big[..], &small[..]], "biggest first");
        let pixels = decode(&file).expect("an icon from the PNG inside");
        assert_eq!(&pixels[(32 * ICON_SIZE + 32) * 4..(32 * ICON_SIZE + 32) * 4 + 4], [0, 0, 255, 255], "red, from the big one");
        let mut cut = file.clone();
        cut.truncate(file.len() - 10);
        assert_eq!(embedded_pngs(&cut), [&small[..]], "a picture that runs past the end is skipped");
        assert!(embedded_pngs(&bmp(2, 2, [0, 0, 0])).is_empty(), "not an icon file");
        assert!(embedded_pngs(&[0, 0, 1, 0, 5, 0]).is_empty(), "entries missing");
    }
}
