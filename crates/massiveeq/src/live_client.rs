//! Coalesced background D-Bus work. GTK never waits for live compilation.
use crate::client::Client;
use massiveeq_core::{ComparisonSet, DeviceInfo, ProfileAnalysis, ProfileInfo};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};

#[derive(Clone, Debug, Serialize)]
pub struct Reference {
    pub text: String,
    pub trim: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Draft {
    #[serde(skip)]
    pub revision: u64,
    #[serde(skip)]
    pub id: String,
    #[serde(skip)]
    pub name: String,
    pub text: String,
    pub trim: f64,
    pub reference: Option<Reference>,
    pub hear_reference: bool,
    /// Output gain the service keeps while the tone generator plays.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold_gain_db: Option<f64>,
    #[serde(skip)]
    pub save: bool,
}

pub struct Completion {
    pub revision: u64,
    pub id: String,
    pub saved: Option<ProfileInfo>,
    pub error: Option<String>,
    pub analysis: Option<ProfileAnalysis>,
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Draft>,
    end: Vec<String>,
    tone: Option<(String, serde_json::Value)>,
    stop_tone: bool,
    stopped: bool,
    barriers: Vec<mpsc::Sender<()>>,
}

impl Queue {
    fn push(&mut self, draft: Draft) {
        if draft.save {
            self.jobs.retain(|job| job.id != draft.id);
            self.jobs.push_back(draft);
        } else if let Some(previous) = self
            .jobs
            .iter_mut()
            .rev()
            .find(|job| job.id == draft.id && !job.save)
        {
            *previous = draft;
        } else {
            self.jobs.push_back(draft);
        }
    }
}

pub struct Poll {
    pub devices: Vec<DeviceInfo>,
    pub comparisons: HashMap<String, ComparisonSet>,
    pub status: serde_json::Value,
}

pub struct LiveClient {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    results: mpsc::Receiver<Completion>,
    poll: Arc<Mutex<Option<Result<Poll, String>>>>,
    stopped: Arc<AtomicBool>,
}

impl LiveClient {
    pub fn new(client: Client) -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let (tx, results) = mpsc::channel();
        let poll = Arc::new(Mutex::new(None));
        let stopped = Arc::new(AtomicBool::new(false));
        let polling = poll.clone();
        let stop = stopped.clone();
        let polling_client = client.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let snapshot = (|| -> anyhow::Result<Poll> {
                    Ok(Poll {
                        devices: polling_client.devices()?,
                        comparisons: polling_client.comparisons()?,
                        status: polling_client.status()?,
                    })
                })();
                *polling.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(snapshot.map_err(|e| format!("{e:#}")));
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        });
        let work = queue.clone();
        std::thread::spawn(move || {
            loop {
                let (lock, ready) = &*work;
                let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
                while state.jobs.is_empty()
                    && state.end.is_empty()
                    && state.tone.is_none()
                    && !state.stop_tone
                    && state.barriers.is_empty()
                    && !state.stopped
                {
                    state = ready.wait(state).unwrap_or_else(|e| e.into_inner());
                }
                let ends = std::mem::take(&mut state.end);
                let stopped = state.stopped;
                let tone = state.tone.take();
                let stop_tone = std::mem::take(&mut state.stop_tone);
                let job = state.jobs.pop_front();
                drop(state);
                for id in ends {
                    let _ = client.end_preview(&id);
                }
                if stop_tone || stopped {
                    let _ = client.stop_tone();
                } else if let Some((device, settings)) = tone
                    && let Err(error) = client.play_tone(&device, &settings)
                {
                    let _ = tx.send(Completion {
                        revision: 0,
                        id: String::new(),
                        saved: None,
                        error: Some(format!("{error:#}")),
                        analysis: None,
                    });
                }
                if stopped {
                    break;
                }
                let Some(job) = job else {
                    let barriers = std::mem::take(
                        &mut lock.lock().unwrap_or_else(|e| e.into_inner()).barriers,
                    );
                    for barrier in barriers {
                        let _ = barrier.send(());
                    }
                    continue;
                };
                let mut error = client
                    .preview_profile(&job.id, &serde_json::to_string(&job).unwrap_or_default())
                    .err()
                    .map(|e| format!("{e:#}"));
                // Save independently even if an output cannot preview (e.g. it is
                // disconnected). Invalid text was filtered at the UI and service.
                let saved = if job.save {
                    match client.put(&job.id, &job.name, &job.text, job.trim) {
                        Ok(profile) => Some(profile),
                        Err(e) => {
                            error = Some(format!("{e:#}"));
                            None
                        }
                    }
                } else {
                    None
                };
                if saved.is_some() && error.is_some() {
                    error = client
                        .preview_profile(&job.id, &serde_json::to_string(&job).unwrap_or_default())
                        .err()
                        .map(|e| format!("{e:#}"));
                }
                let analysis = if saved.is_some() {
                    client.analyze(&job.id).ok()
                } else {
                    None
                };
                if tx
                    .send(Completion {
                        revision: job.revision,
                        id: job.id,
                        saved,
                        error,
                        analysis,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            queue,
            results,
            poll,
            stopped,
        }
    }

    pub fn submit(&self, draft: Draft) {
        let (lock, ready) = &*self.queue;
        lock.lock().unwrap_or_else(|e| e.into_inner()).push(draft);
        ready.notify_one();
    }

    pub fn end(&self, id: &str) {
        let (lock, ready) = &*self.queue;
        let mut queue = lock.lock().unwrap_or_else(|e| e.into_inner());
        // Finish any queued save before ending. A draft-only request can go.
        queue.jobs.retain(|job| job.id != id || job.save);
        for job in queue.jobs.iter_mut().filter(|job| job.id == id) {
            job.reference = None;
            job.hear_reference = false;
        }
        queue.end.push(id.to_owned());
        ready.notify_one();
    }

    pub fn tone(&self, key: String, settings: serde_json::Value) {
        let (lock, ready) = &*self.queue;
        let mut queue = lock.lock().unwrap_or_else(|e| e.into_inner());
        queue.tone = Some((key, settings));
        queue.stop_tone = false;
        ready.notify_one();
    }

    pub fn stop_tone(&self) {
        let (lock, ready) = &*self.queue;
        let mut queue = lock.lock().unwrap_or_else(|e| e.into_inner());
        queue.tone = None;
        queue.stop_tone = true;
        ready.notify_one();
    }

    pub fn flush(&self) -> bool {
        let (tx, rx) = mpsc::channel();
        let (lock, ready) = &*self.queue;
        lock.lock()
            .unwrap_or_else(|e| e.into_inner())
            .barriers
            .push(tx);
        ready.notify_one();
        rx.recv_timeout(std::time::Duration::from_secs(2)).is_ok()
    }

    pub fn take_poll(&self) -> Option<Result<Poll, String>> {
        self.poll.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    pub fn completions(&self) -> Vec<Completion> {
        self.results.try_iter().collect()
    }
}

impl Drop for LiveClient {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        let (lock, ready) = &*self.queue;
        lock.lock().unwrap_or_else(|e| e.into_inner()).stopped = true;
        ready.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn draft(text: &str, save: bool) -> Draft {
        Draft {
            revision: 1,
            id: "profile".into(),
            name: "Profile".into(),
            text: text.into(),
            trim: 0.0,
            reference: None,
            hear_reference: false,
            hold_gain_db: None,
            save,
        }
    }
    #[test]
    fn a_new_drag_does_not_overwrite_a_released_snapshot_waiting_to_save() {
        let mut queue = Queue::default();
        queue.push(draft("released edit", true));
        queue.push(draft("new gesture", false));
        let saved = queue.jobs.iter().find(|job| job.save).unwrap();
        assert_eq!(
            saved.text, "released edit",
            "only released/idle snapshots may be saved"
        );
        assert!(
            queue
                .jobs
                .iter()
                .any(|job| !job.save && job.text == "new gesture")
        );
    }
}
