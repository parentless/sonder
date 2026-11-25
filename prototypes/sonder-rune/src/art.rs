//! Album covers: asked for while a view draws, fetched off-thread, decoded small,
//! and kept under Sonder's 128-cover budget so scrolling a huge library cannot
//! grow memory. Originals are not kept: the disk cache holds them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use rune::{Image, ImageCache, Pixels, Waker};

/// Covers drawn at most 56 pt wide are decoded at 112 px; the big one for Now Playing is separate.
const THUMB_PT: f32 = 56.0;
const BIG_PT: f32 = 360.0;
const SCALE: f32 = 2.0;
/// The most covers kept decoded; past it the least recently drawn go.
const KEEP: usize = 128;

enum Slot {
    Wanted,
    Failed,
    Ready { image: Image, used: u64 },
}

pub struct Art {
    cache: ImageCache,
    slots: RefCell<HashMap<String, Slot>>,
    wanted: RefCell<Vec<String>>,
    tick: RefCell<u64>,
    /// The raw bytes of the one cover Now Playing shows large.
    pinned: Option<(String, Arc<[u8]>)>,
    pin_next: Option<String>,
    big: RefCell<Option<(String, Image)>>,
}

impl Art {
    pub fn new(waker: Waker) -> Art {
        Art {
            cache: ImageCache::new(waker),
            slots: RefCell::default(),
            wanted: RefCell::default(),
            tick: RefCell::new(0),
            pinned: None,
            pin_next: None,
            big: RefCell::new(None),
        }
    }

    /// The cover as pixels once it has loaded. The first ask queues the fetch.
    pub fn get(&self, url: &str) -> Option<Pixels> {
        let mut slots = self.slots.borrow_mut();
        let mut tick = self.tick.borrow_mut();
        *tick += 1;
        match slots.get_mut(url) {
            Some(Slot::Ready { image, used }) => {
                *used = *tick;
                Some(image.pixels().clone())
            }
            Some(_) => None,
            None => {
                slots.insert(url.to_owned(), Slot::Wanted);
                self.wanted.borrow_mut().push(url.to_owned());
                None
            }
        }
    }

    /// The big cover for Now Playing, from the bytes pinned for it.
    pub fn big(&self, url: &str) -> Option<Pixels> {
        let mut big = self.big.borrow_mut();
        if let Some((have, image)) = big.as_ref() {
            if have == url {
                return Some(image.pixels().clone());
            }
        }
        let (pinned, bytes) = self.pinned.as_ref()?;
        if pinned != url {
            return None;
        }
        let image = self.cache.load_scaled(bytes.clone(), BIG_PT, SCALE);
        let pixels = image.pixels().clone();
        *big = Some((url.to_owned(), image));
        Some(pixels)
    }

    /// Keeps this cover's bytes for the big picture, dropping the previous one's.
    pub fn pin(&mut self, url: &str) {
        if self.pinned.as_ref().is_some_and(|(have, _)| have == url) {
            return;
        }
        self.pinned = None;
        *self.big.borrow_mut() = None;
        // Asking again re-fetches the bytes and pins them in `supply`.
        self.slots.borrow_mut().remove(url);
        self.pin_next = Some(url.to_owned());
    }

    pub fn take_wanted(&self) -> Vec<String> {
        std::mem::take(&mut *self.wanted.borrow_mut())
    }

    /// A fetched cover arrived (or failed).
    pub fn supply(&mut self, url: String, bytes: Option<Vec<u8>>) {
        let mut slots = self.slots.borrow_mut();
        let Some(bytes) = bytes.map(Arc::<[u8]>::from) else {
            slots.insert(url, Slot::Failed);
            return;
        };
        let image = self.cache.load_scaled(bytes.clone(), THUMB_PT, SCALE);
        if self.pin_next.as_deref() == Some(url.as_str()) {
            self.pin_next = None;
            self.pinned = Some((url.clone(), bytes));
        }
        let used = *self.tick.borrow();
        slots.insert(url, Slot::Ready { image, used });
        if slots.len() > KEEP {
            let mut ready: Vec<(String, u64)> = slots
                .iter()
                .filter_map(|(url, slot)| match slot {
                    Slot::Ready { used, .. } => Some((url.clone(), *used)),
                    _ => None,
                })
                .collect();
            ready.sort_by_key(|(_, used)| *used);
            for (url, _) in ready.into_iter().take(slots.len() - KEEP * 3 / 4) {
                slots.remove(&url);
            }
        }
    }
}
