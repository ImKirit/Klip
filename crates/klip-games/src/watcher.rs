//! The watcher: tells the recorder when a game starts, comes to the front, goes to the
//! back and ends.
//!
//! It polls instead of hooking. A WinEvent hook for foreground changes would also work
//! from outside, but a poll every two seconds is simpler, needs no message loop and costs
//! next to nothing: one tick is two system calls as long as the foreground window stays
//! the same. The expensive part (exe path, catalog lookup) only runs when it changes.

use crate::catalog::{GameCatalog, Identity};
use crate::foreground::{self, ForegroundInfo};
use crate::library::GameSource;
use crate::protection::Protection;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// A game that is running right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunningGame {
    /// Key for profiles.
    pub id: String,
    pub name: String,
    pub source: GameSource,
    pub pid: u32,
    /// `None` if Windows refused the path, then the game was recognised by name.
    pub exe_path: Option<PathBuf>,
    pub exe_name: String,
    pub protection: Protection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameEvent {
    /// A game was seen for the first time. It is seen when it first comes to the
    /// foreground, a game that starts hidden is not noticed before that.
    Started(RunningGame),
    /// The game has the focus. Always follows `Started`.
    Foreground(RunningGame),
    /// The game lost the focus but still runs.
    Background(RunningGame),
    /// The process is gone.
    Stopped(RunningGame),
    /// An unknown program covers a whole monitor. The app may ask "is this a game?" and
    /// put the answer into the manual list. Reported once per window, never recorded
    /// on its own.
    UnknownFullscreen(ForegroundInfo),
}

#[derive(Debug, Clone)]
pub struct WatcherOptions {
    /// Time between two looks at the foreground window.
    pub interval: Duration,
    /// Report unknown fullscreen programs.
    pub fullscreen_heuristic: bool,
}

impl Default for WatcherOptions {
    fn default() -> Self {
        Self { interval: Duration::from_secs(2), fullscreen_heuristic: true }
    }
}

/// Counters for measuring what the watcher costs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WatcherStats {
    pub ticks: u64,
    /// Ticks in which the foreground program had to be resolved again.
    pub resolves: u64,
    /// Time spent working, summed over all ticks.
    pub busy: Duration,
}

#[derive(Default)]
struct Counters {
    ticks: AtomicU64,
    resolves: AtomicU64,
    busy_nanos: AtomicU64,
}

enum Control {
    Catalog(Box<GameCatalog>),
    Stop,
}

/// Handle to the watcher thread. Dropping it stops the thread.
pub struct GameWatcher {
    control: Sender<Control>,
    thread: Option<JoinHandle<()>>,
    counters: Arc<Counters>,
}

impl GameWatcher {
    /// Starts the watcher. Events arrive on the returned channel; when the receiver is
    /// dropped the thread ends by itself.
    pub fn start(catalog: GameCatalog, options: WatcherOptions) -> (Self, Receiver<GameEvent>) {
        let (control, control_rx) = mpsc::channel();
        let (events, events_rx) = mpsc::channel();
        let counters = Arc::new(Counters::default());
        let thread_counters = counters.clone();
        let thread = std::thread::Builder::new()
            .name("klip-game-watcher".into())
            .spawn(move || {
                let mut state = State::new(catalog, options.fullscreen_heuristic);
                loop {
                    let started = Instant::now();
                    let resolved = match state.tick(&events) {
                        Ok(resolved) => resolved,
                        Err(()) => break,
                    };
                    thread_counters.ticks.fetch_add(1, Ordering::Relaxed);
                    thread_counters.resolves.fetch_add(resolved as u64, Ordering::Relaxed);
                    thread_counters.busy_nanos.fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
                    // Sleeping in recv_timeout: the thread is parked by the kernel and wakes
                    // for the next tick or at once for a command.
                    match control_rx.recv_timeout(options.interval) {
                        Err(RecvTimeoutError::Timeout) => {}
                        Ok(Control::Catalog(catalog)) => state.replace_catalog(*catalog),
                        Ok(Control::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .expect("spawning the watcher thread");
        (Self { control, thread: Some(thread), counters }, events_rx)
    }

    /// Hands the watcher a new catalog (after a rescan or a change of the manual list).
    /// The foreground program is judged again with it on the next tick.
    pub fn update_catalog(&self, catalog: GameCatalog) {
        let _ = self.control.send(Control::Catalog(Box::new(catalog)));
    }

    pub fn stats(&self) -> WatcherStats {
        WatcherStats {
            ticks: self.counters.ticks.load(Ordering::Relaxed),
            resolves: self.counters.resolves.load(Ordering::Relaxed),
            busy: Duration::from_nanos(self.counters.busy_nanos.load(Ordering::Relaxed)),
        }
    }

    /// Stops the thread and waits for it.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let _ = self.control.send(Control::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for GameWatcher {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// An unknown foreground program that may still grow to fullscreen.
struct Candidate {
    info: ForegroundInfo,
    reported: bool,
}

struct State {
    catalog: GameCatalog,
    fullscreen_heuristic: bool,
    /// Window and process seen on the last tick.
    last: Option<(isize, u32)>,
    /// Every game seen and not yet ended.
    running: Vec<RunningGame>,
    /// Process id of the game that has the focus.
    front: Option<u32>,
    candidate: Option<Candidate>,
}

impl State {
    fn new(catalog: GameCatalog, fullscreen_heuristic: bool) -> Self {
        Self { catalog, fullscreen_heuristic, last: None, running: Vec::new(), front: None, candidate: None }
    }

    fn replace_catalog(&mut self, catalog: GameCatalog) {
        self.catalog = catalog;
        // Forget the last window so the next tick resolves it with the new catalog.
        self.last = None;
    }

    /// One look at the system. `Ok(true)` if the foreground program was resolved again,
    /// `Err` if nobody listens any more.
    fn tick(&mut self, events: &Sender<GameEvent>) -> Result<bool, ()> {
        let send = |event| events.send(event).map_err(|_| ());
        let now = foreground::foreground_ids();
        let changed = now != self.last;
        if changed {
            self.last = now;
            self.candidate = None;
            let mut front_now = None;
            if let Some((hwnd, pid)) = now {
                let info = foreground::resolve_foreground(hwnd, pid);
                let name = info.exe_name.clone().unwrap_or_default();
                match self.catalog.identify(info.exe_path.as_deref(), &name) {
                    Identity::Game(game) => {
                        front_now = Some(pid);
                        // Same id, other exe: the process id was reused by another game.
                        if let Some(old) = self.running.iter().position(|g| g.pid == pid && g.id != game.id) {
                            let old = self.running.remove(old);
                            if self.front == Some(pid) {
                                self.front = None;
                            }
                            send(GameEvent::Stopped(old))?;
                        }
                        if !self.running.iter().any(|g| g.pid == pid) {
                            let running = RunningGame {
                                id: game.id,
                                name: game.name,
                                source: game.source,
                                pid,
                                exe_path: info.exe_path.clone(),
                                exe_name: name,
                                protection: game.protection,
                            };
                            self.running.push(running.clone());
                            send(GameEvent::Started(running))?;
                        }
                    }
                    Identity::Unknown(_) if !name.is_empty() => {
                        self.candidate = Some(Candidate { info, reported: false });
                    }
                    Identity::Unknown(_) | Identity::Excluded => {}
                }
            }
            if front_now != self.front {
                if let Some(old) = self.front.and_then(|pid| self.running.iter().find(|g| g.pid == pid)) {
                    send(GameEvent::Background(old.clone()))?;
                }
                if let Some(new) = front_now.and_then(|pid| self.running.iter().find(|g| g.pid == pid)) {
                    send(GameEvent::Foreground(new.clone()))?;
                }
                self.front = front_now;
            }
        }

        // An unknown window can switch to fullscreen without the foreground changing, so
        // its rectangle is looked at on every tick until it has been reported once.
        if self.fullscreen_heuristic {
            if let Some(candidate) = self.candidate.as_mut().filter(|c| !c.reported) {
                if foreground::covers_monitor(candidate.info.hwnd) {
                    candidate.reported = true;
                    candidate.info.covers_monitor = true;
                    send(GameEvent::UnknownFullscreen(candidate.info.clone()))?;
                }
            }
        }

        // Games end without telling anyone. Only games already seen are checked, and each
        // check is one limited query on that process id.
        let mut i = 0;
        while i < self.running.len() {
            let game = &self.running[i];
            if foreground::is_alive(game.pid, game.exe_path.as_deref(), &game.exe_name) {
                i += 1;
                continue;
            }
            let gone = self.running.remove(i);
            if self.front == Some(gone.pid) {
                self.front = None;
            }
            send(GameEvent::Stopped(gone))?;
        }
        Ok(changed)
    }
}
