use crate::api::http::HttpHeader;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct Cached {
    pub headers: Vec<HttpHeader>,
    pub status: u16,
    pub url: String,
    pub body: Vec<u8>,
    time: Instant,
}
struct Cache {
    entries: VecDeque<(String, Cached)>,
    size: usize,
}
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
fn cache() -> &'static Mutex<Cache> {
    CACHE.get_or_init(|| {
        Mutex::new(Cache {
            entries: VecDeque::new(),
            size: 0,
        })
    })
}
pub fn clear() {
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    c.entries.clear();
    c.size = 0;
}

pub fn remove(key: &str) {
    let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(index) = cache.entries.iter().position(|(k, _)| k == key) {
        cache.size -= cache.entries.remove(index).unwrap().1.body.len();
    }
}
pub fn get(key: &str, long: bool) -> Option<Cached> {
    let c = cache().lock().unwrap_or_else(|e| e.into_inner());
    c.entries
        .iter()
        .find(|(k, v)| {
            k == key && v.time.elapsed() < Duration::from_secs(if long { 21600 } else { 5 })
        })
        .map(|(_, v)| v.clone())
}
pub fn put(key: String, headers: Vec<HttpHeader>, status: u16, url: String, body: Vec<u8>) {
    if status != 200
        || body.is_empty()
        || body.len() > 1024 * 1024
        || headers.iter().any(|h| {
            h.name.eq_ignore_ascii_case("cache-control")
                && h.value.to_lowercase().contains("no-store")
        })
    {
        return;
    }
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(index) = c.entries.iter().position(|(k, _)| k == &key) {
        c.size -= c.entries.remove(index).unwrap().1.body.len();
    }
    while c.size + body.len() > 10 * 1024 * 1024 {
        if let Some((_, old)) = c.entries.pop_front() {
            c.size -= old.body.len();
        } else {
            break;
        }
    }
    c.size += body.len();
    c.entries.push_back((
        key,
        Cached {
            headers,
            status,
            url,
            body,
            time: Instant::now(),
        },
    ));
}
pub fn key(url: &str, headers: &[HttpHeader], partition: &str) -> String {
    let mut headers: Vec<_> = headers
        .iter()
        .filter(|h| !is_policy(&h.name))
        .map(|h| (h.name.to_ascii_lowercase(), h.value.clone()))
        .collect();
    headers.sort();
    // Includes Authorization, Cookie and custom tokens; cache entries never cross sessions.
    format!("{url}|{partition}|{headers:?}")
}
pub fn is_policy(name: &str) -> bool {
    ["cache-time", "prevent-parallel", "http_client"]
        .iter()
        .any(|n| name.eq_ignore_ascii_case(n))
}
pub fn header<'a>(headers: &'a [HttpHeader], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}
pub async fn serial(url: &str) -> tokio::sync::OwnedMutexGuard<()> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let mutex = {
        let mut locks = LOCKS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        locks.retain(|_, v| v.strong_count() > 0);
        if let Some(lock) = locks.get(url).and_then(Weak::upgrade) {
            lock
        } else {
            let lock = Arc::new(tokio::sync::Mutex::new(()));
            locks.insert(url.into(), Arc::downgrade(&lock));
            lock
        }
    };
    mutex.lock_owned().await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_isolates_credentials_and_honors_no_store() {
        let a = key(
            "test",
            &[HttpHeader {
                name: "Authorization".into(),
                value: "a".into(),
            }],
            "",
        );
        let b = key(
            "test",
            &[HttpHeader {
                name: "authorization".into(),
                value: "b".into(),
            }],
            "",
        );
        assert_ne!(a, b);
        put(a.clone(), vec![], 200, "test".into(), vec![1]);
        assert!(get(&a, false).is_some());
        assert!(get(&b, false).is_none());
        put(
            b.clone(),
            vec![HttpHeader {
                name: "cache-control".into(),
                value: "no-store".into(),
            }],
            200,
            "test".into(),
            vec![1],
        );
        assert!(get(&b, false).is_none());
    }
}
