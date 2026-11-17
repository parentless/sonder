//! The songs being saved from links, which outlive the popover that started
//! them. The work runs on plain threads (see `crate::link_download`); this
//! keeps what the popover shows and tells the library when a file landed.
use super::*;
use crate::link_download::{self, Format, Job, Probe, Update};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// Downloads running at once; the rest wait their turn.
const MAX_RUNNING_JOBS: usize = 2;
/// Links looked up at once.
const MAX_RUNNING_PROBES: usize = 3;
/// Finished downloads kept in the list.
const KEPT_FINISHED: usize = 6;
/// Progress reaches the screen at most this often.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(120);

/// What Sonder knows about a link the listener pasted.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum ProbeState {
    /// Being looked up. `setup` is set while a tool is fetched for the first
    /// time, which is the slow part.
    Looking {
        setup: Option<(&'static str, Option<f32>)>,
    },
    Found(Probe),
    Failed(String),
}

/// Where a download is.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Phase {
    Queued,
    Setup {
        what: &'static str,
        fraction: Option<f32>,
    },
    Working {
        fraction: Option<f32>,
        bytes_per_second: Option<f64>,
        item: Option<(usize, usize)>,
    },
    Converting,
    /// How many files it made.
    Done(usize),
    Failed(String),
    Cancelled,
}

impl Phase {
    pub(super) fn active(&self) -> bool {
        !matches!(self, Self::Done(_) | Self::Failed(_) | Self::Cancelled)
    }
}

/// One download in the list.
pub(super) struct Item {
    pub id: u64,
    pub url: String,
    pub probe: Probe,
    pub format: Format,
    pub folder: PathBuf,
    pub playlist: bool,
    pub phase: Phase,
    cancel: Arc<AtomicBool>,
}

impl Item {
    pub(super) fn title(&self) -> &str {
        &self.probe.title
    }
}

enum Message {
    Probing {
        url: String,
        setup: Option<(&'static str, Option<f32>)>,
    },
    Probed {
        url: String,
        result: Result<Probe, String>,
    },
    Progress {
        id: u64,
        update: Update,
    },
    Finished {
        id: u64,
        result: Result<Vec<PathBuf>, String>,
    },
}

pub(super) struct LinkDownloads {
    probes: HashMap<String, ProbeState>,
    probe_cancels: HashMap<String, Arc<AtomicBool>>,
    waiting_probes: VecDeque<String>,
    running_probes: usize,
    items: Vec<Item>,
    next_id: u64,
    format: Format,
    folder: Option<PathBuf>,
    sender: async_channel::Sender<Message>,
    _pump: gpui_kit::Task<()>,
}

impl LinkDownloads {
    pub(super) fn new(choice: (Option<String>, Option<PathBuf>), cx: &mut Context<Self>) -> Self {
        let (sender, receiver) = async_channel::unbounded::<Message>();
        let pump = cx.spawn(async move |this, cx| {
            while let Ok(message) = receiver.recv().await {
                if this
                    .update(cx, |downloads, cx| downloads.apply(message, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let (format, folder) = choice;
        Self {
            probes: HashMap::new(),
            probe_cancels: HashMap::new(),
            waiting_probes: VecDeque::new(),
            running_probes: 0,
            items: Vec::new(),
            next_id: 1,
            format: format
                .as_deref()
                .and_then(Format::from_key)
                .unwrap_or(Format::Best),
            folder,
            sender,
            _pump: pump,
        }
    }

    pub(super) fn format(&self) -> Format {
        self.format
    }

    /// The folder a save goes to: the last one chosen if it is still one of
    /// `folders`, otherwise the first.
    pub(super) fn folder(&self, folders: &[PathBuf]) -> Option<PathBuf> {
        self.folder
            .clone()
            .filter(|chosen| folders.contains(chosen))
            .or_else(|| folders.first().cloned())
    }

    pub(super) fn choose(
        &mut self,
        format: Format,
        folder: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.format = format;
        if folder.is_some() {
            self.folder = folder;
        }
        services::AppServices::set_link_download_choice(format.key(), self.folder.as_deref(), cx);
        cx.notify();
    }

    pub(super) fn probe(&self, url: &str) -> Option<&ProbeState> {
        self.probes.get(url)
    }

    pub(super) fn items(&self) -> &[Item] {
        &self.items
    }

    /// Looks up each link not yet known, and drops lookups for links no
    /// longer in the field so a half-typed address does not keep working.
    pub(super) fn look_up(&mut self, links: &[String], cx: &mut Context<Self>) {
        let stale: Vec<String> = self
            .probes
            .iter()
            .filter(|(url, state)| {
                !links.contains(url) && matches!(state, ProbeState::Looking { .. })
            })
            .map(|(url, _)| url.clone())
            .collect();
        for url in stale {
            if let Some(cancel) = self.probe_cancels.get(&url) {
                cancel.store(true, Ordering::Relaxed);
            }
            self.waiting_probes.retain(|waiting| *waiting != url);
            self.probes.remove(&url);
        }
        for link in links {
            if !self.probes.contains_key(link) {
                self.probes
                    .insert(link.clone(), ProbeState::Looking { setup: None });
                self.waiting_probes.push_back(link.clone());
            }
        }
        self.start_probes();
        cx.notify();
    }

    /// Looks a failed link up again.
    pub(super) fn retry_probe(&mut self, url: &str, cx: &mut Context<Self>) {
        if matches!(self.probes.get(url), Some(ProbeState::Failed(_))) {
            self.probes.remove(url);
            self.look_up(&[url.to_owned()], cx);
        }
    }

    fn start_probes(&mut self) {
        while self.running_probes < MAX_RUNNING_PROBES {
            let Some(url) = self.waiting_probes.pop_front() else {
                return;
            };
            self.running_probes += 1;
            let cancel = Arc::new(AtomicBool::new(false));
            self.probe_cancels.insert(url.clone(), cancel.clone());
            let sender = self.sender.clone();
            std::thread::spawn(move || {
                let progress_url = url.clone();
                let progress_sender = sender.clone();
                let result = link_download::probe(&url, &cancel, &mut |update| {
                    if let Update::Setup { what, fraction } = update {
                        let _ = progress_sender.send_blocking(Message::Probing {
                            url: progress_url.clone(),
                            setup: Some((what, fraction)),
                        });
                    }
                });
                let _ = sender.send_blocking(Message::Probed {
                    url,
                    result: result.map_err(|error| error.to_string()),
                });
            });
        }
    }

    /// Queues a save for every link that has been looked up.
    /// `playlists` lists the links whose whole playlist should be taken.
    pub(super) fn save(
        &mut self,
        links: &[String],
        playlists: &[String],
        folder: PathBuf,
        cx: &mut Context<Self>,
    ) {
        for link in links {
            let Some(ProbeState::Found(probe)) = self.probes.get(link).cloned() else {
                continue;
            };
            let id = self.next_id;
            self.next_id += 1;
            self.items.push(Item {
                id,
                url: link.clone(),
                probe,
                format: self.format,
                folder: folder.clone(),
                playlist: playlists.contains(link),
                phase: Phase::Queued,
                cancel: Arc::new(AtomicBool::new(false)),
            });
        }
        self.start_jobs();
        self.trim_finished();
        cx.notify();
    }

    fn start_jobs(&mut self) {
        let mut running = self
            .items
            .iter()
            .filter(|item| !matches!(item.phase, Phase::Queued) && item.phase.active())
            .count();
        for item in &mut self.items {
            if running >= MAX_RUNNING_JOBS {
                break;
            }
            if !matches!(item.phase, Phase::Queued) {
                continue;
            }
            running += 1;
            item.phase = Phase::Working {
                fraction: None,
                bytes_per_second: None,
                item: None,
            };
            let job = Job {
                url: item.url.clone(),
                format: item.format,
                folder: item.folder.clone(),
                playlist: item.playlist,
                probe: item.probe.clone(),
            };
            let id = item.id;
            let cancel = item.cancel.clone();
            let sender = self.sender.clone();
            std::thread::spawn(move || {
                let mut last_sent: Option<Instant> = None;
                let result = link_download::save(&job, &cancel, &mut |update| {
                    let due = last_sent.is_none_or(|sent| sent.elapsed() >= PROGRESS_INTERVAL)
                        || !matches!(update, Update::Working { .. });
                    if due {
                        last_sent = Some(Instant::now());
                        let _ = sender.send_blocking(Message::Progress { id, update });
                    }
                });
                let _ = sender.send_blocking(Message::Finished {
                    id,
                    result: result.map_err(|error| error.to_string()),
                });
            });
        }
    }

    /// Stops a download, or removes a finished one from the list.
    pub(super) fn dismiss(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(position) = self.items.iter().position(|item| item.id == id) else {
            return;
        };
        let item = &mut self.items[position];
        if item.phase.active() {
            item.cancel.store(true, Ordering::Relaxed);
            if matches!(item.phase, Phase::Queued) {
                item.phase = Phase::Cancelled;
            }
        } else {
            self.items.remove(position);
        }
        cx.notify();
    }

    /// Tries a failed or cancelled download again.
    pub(super) fn retry(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id)
            && matches!(item.phase, Phase::Failed(_) | Phase::Cancelled)
        {
            item.cancel = Arc::new(AtomicBool::new(false));
            item.phase = Phase::Queued;
            self.start_jobs();
            cx.notify();
        }
    }

    fn trim_finished(&mut self) {
        let mut finished = self
            .items
            .iter()
            .filter(|item| !item.phase.active())
            .count();
        self.items.retain(|item| {
            if item.phase.active() || finished <= KEPT_FINISHED {
                true
            } else {
                finished -= 1;
                false
            }
        });
    }

    fn apply(&mut self, message: Message, cx: &mut Context<Self>) {
        match message {
            Message::Probing { url, setup } => {
                if let Some(state @ ProbeState::Looking { .. }) = self.probes.get_mut(&url) {
                    *state = ProbeState::Looking { setup };
                }
            }
            Message::Probed { url, result } => {
                self.running_probes = self.running_probes.saturating_sub(1);
                self.probe_cancels.remove(&url);
                // A lookup dropped from the field while it ran stays dropped.
                if matches!(self.probes.get(&url), Some(ProbeState::Looking { .. })) {
                    self.probes.insert(
                        url,
                        match result {
                            Ok(probe) => ProbeState::Found(probe),
                            Err(message) => ProbeState::Failed(message),
                        },
                    );
                }
                self.start_probes();
            }
            Message::Progress { id, update } => {
                if let Some(item) = self.items.iter_mut().find(|item| item.id == id)
                    && item.phase.active()
                    && !item.cancel.load(Ordering::Relaxed)
                {
                    item.phase = match update {
                        Update::Setup { what, fraction } => Phase::Setup { what, fraction },
                        Update::Working {
                            fraction,
                            bytes_per_second,
                            item,
                        } => Phase::Working {
                            fraction,
                            bytes_per_second,
                            item,
                        },
                        Update::Converting => Phase::Converting,
                    };
                }
            }
            Message::Finished { id, result } => {
                if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
                    item.phase = match result {
                        Ok(files) => Phase::Done(files.len()),
                        Err(_) if item.cancel.load(Ordering::Relaxed) => Phase::Cancelled,
                        Err(message) => Phase::Failed(message),
                    };
                    if matches!(item.phase, Phase::Done(_)) {
                        // The folder watcher usually notices; this covers
                        // folders it could not watch.
                        services::AppServices::library(cx)
                            .update(cx, |library, cx| library.rescan_after_change(cx));
                    }
                }
                self.start_jobs();
                self.trim_finished();
            }
        }
        cx.notify();
    }

    #[cfg(test)]
    pub(super) fn fixture_probe(&mut self, url: &str, state: ProbeState) {
        self.probes.insert(url.to_owned(), state);
    }

    #[cfg(test)]
    pub(super) fn fixture_item(&mut self, url: &str, probe: Probe, folder: PathBuf, phase: Phase) {
        let id = self.next_id;
        self.next_id += 1;
        self.items.push(Item {
            id,
            url: url.to_owned(),
            probe,
            format: self.format,
            folder,
            playlist: false,
            phase,
            cancel: Arc::new(AtomicBool::new(false)),
        });
    }
}

/// A one-line status for the list: what is happening, in words.
pub(super) fn phase_label(phase: &Phase) -> SharedString {
    match phase {
        Phase::Queued => "Waiting".into(),
        Phase::Setup { what, fraction } => match fraction {
            Some(fraction) => format!(
                "Getting {what} (one time) · {}%",
                (fraction * 100.).round() as u32
            ),
            None => format!("Getting {what} (one time)"),
        }
        .into(),
        Phase::Working {
            fraction,
            bytes_per_second,
            item,
        } => {
            let mut text = match fraction {
                Some(fraction) => format!("{}%", (fraction * 100.).round() as u32),
                None => "Starting".to_owned(),
            };
            if let Some((index, total)) = item {
                text = format!("Song {index} of {total} · {text}");
            }
            if let Some(rate) = bytes_per_second.filter(|rate| *rate > 1024.) {
                text.push_str(&format!(" · {:.1} MB/s", rate / (1024. * 1024.)));
            }
            text.into()
        }
        Phase::Converting => "Converting and tagging".into(),
        Phase::Done(1) => "Added".into(),
        Phase::Done(count) => format!("{count} songs added").into(),
        Phase::Failed(message) => message.clone().into(),
        Phase::Cancelled => "Cancelled".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_line_names_what_is_happening() {
        assert_eq!(phase_label(&Phase::Queued), "Waiting");
        assert_eq!(
            phase_label(&Phase::Working {
                fraction: Some(0.64),
                bytes_per_second: Some(1.1 * 1024. * 1024.),
                item: None
            }),
            "64% · 1.1 MB/s"
        );
        assert_eq!(
            phase_label(&Phase::Working {
                fraction: None,
                bytes_per_second: None,
                item: Some((3, 12))
            }),
            "Song 3 of 12 · Starting"
        );
        assert_eq!(phase_label(&Phase::Done(1)), "Added");
        assert_eq!(phase_label(&Phase::Done(4)), "4 songs added");
        assert!(
            phase_label(&Phase::Setup {
                what: "the downloader",
                fraction: Some(0.5)
            })
            .contains("one time")
        );
    }

    #[test]
    fn only_unfinished_phases_are_active() {
        assert!(Phase::Queued.active());
        assert!(Phase::Converting.active());
        assert!(!Phase::Done(1).active());
        assert!(!Phase::Failed("x".into()).active());
        assert!(!Phase::Cancelled.active());
    }
}
