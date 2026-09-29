//! File icons for result rows.
//!
//! Rendering an icon takes 1-20ms, so rows are never held back for one: icons
//! already rendered are returned with the row, the rest are rendered on a
//! small dedicated pool and delivered through `icon_update` events.

use crate::background::IconPayload;
use base64::{Engine as _, engine::general_purpose};
use crossbeam_channel::Sender;
use parking_lot::Mutex;
use rayon::{ThreadPool, ThreadPoolBuilder};
use search_cache::SlabIndex;
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
};

/// Kept small so icon work never competes with searches for the global pool.
static POOL: LazyLock<ThreadPool> = LazyLock::new(|| {
    ThreadPoolBuilder::new()
        .num_threads(4)
        .thread_name(|i| format!("cardinal-icon-{i}"))
        .build()
        .expect("Failed to build icon thread pool")
});

/// Rendered Finder icons by path, as data URLs (~3KB each).
static CACHE: LazyLock<Mutex<HashMap<String, Arc<str>>>> = LazyLock::new(Default::default);
const MAX_CACHED: usize = 4096;

/// Runs icon work on the icon pool.
pub(crate) fn spawn(job: impl FnOnce() + Send + 'static) {
    POOL.spawn(job);
}

pub(crate) fn png_data_url(png: &[u8]) -> String {
    format!(
        "data:image/png;base64,{}",
        general_purpose::STANDARD.encode(png)
    )
}

/// Returns the Finder icon of `path` if it's rendered already; otherwise
/// renders it in the background and sends it for `slab_index` to `icon_tx`.
pub(crate) fn cached_or_request(
    slab_index: SlabIndex,
    path: &str,
    icon_tx: &Sender<IconPayload>,
) -> Option<Arc<str>> {
    if let Some(icon) = CACHE.lock().get(path) {
        return Some(icon.clone());
    }
    let path = path.to_string();
    let icon_tx = icon_tx.clone();
    spawn(move || {
        if let Some(icon) = finder_icon(&path) {
            let _ = icon_tx.send(IconPayload {
                slab_index,
                icon: icon.to_string(),
            });
        }
    });
    None
}

/// The Finder icon of `path`, rendered at most once while cached.
pub(crate) fn finder_icon(path: &str) -> Option<Arc<str>> {
    if let Some(icon) = CACHE.lock().get(path) {
        return Some(icon.clone());
    }
    let icon: Arc<str> = png_data_url(&fs_icon::icon_of_path_ns(path)?).into();
    let mut cache = CACHE.lock();
    if cache.len() >= MAX_CACHED {
        cache.clear();
    }
    cache.insert(path.to_string(), icon.clone());
    Some(icon)
}
