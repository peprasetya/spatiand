//! Everything that waits on a network or a disk, kept off the thread that draws.
//!
//! A job is a closure and its answer is a [`Done`], handed back on a channel the main loop
//! looks at between pictures. Two threads: one for what the wearer is waiting on -- a
//! folder's list, a sign-in -- and one for pictures, so that forty posters arriving do not
//! stand between a press and its folder. Telling a server where a film has got to has a
//! third, so that a server slow to answer delays neither.

use std::sync::mpsc::{channel, Receiver, Sender};

use spatiand_media::still::Still;

use crate::source::{dlna, Item};

/// What a job came to.
pub enum Done {
    /// A folder's contents. `ticket` is which asking this answers: an answer to a folder
    /// since left is dropped.
    Listed { ticket: u64, result: Result<Vec<Item>, String> },
    /// A tile's picture.
    Pictured { ticket: u64, index: usize, still: Option<Still> },
    /// The part-watched films of one source, for the home screen: which source, by its place.
    Unfinished { ticket: u64, source: usize, items: Vec<Item> },
    /// A sign-in finished, one way or the other.
    SignedIn(Result<crate::Account, String>),
    /// Servers on the network that answered.
    Found(Vec<dlna::Found>),
}

type Job = Box<dyn FnOnce() -> Option<Done> + Send>;

pub struct Work {
    calls: Sender<Job>,
    pictures: Sender<Job>,
    reports: Sender<Job>,
    pub done: Receiver<Done>,
}

fn worker(name: &str, done: Sender<Done>) -> Sender<Job> {
    let (jobs, queue) = channel::<Job>();
    let _ = std::thread::Builder::new().name(name.into()).spawn(move || {
        for job in queue {
            if let Some(answer) = job() {
                if done.send(answer).is_err() {
                    return;
                }
            }
        }
    });
    jobs
}

impl Work {
    pub fn new() -> Work {
        let (done, answers) = channel();
        Work { calls: worker("calls", done.clone()), pictures: worker("pictures", done.clone()), reports: worker("reports", done), done: answers }
    }

    pub fn call(&self, job: impl FnOnce() -> Option<Done> + Send + 'static) {
        let _ = self.calls.send(Box::new(job));
    }

    pub fn picture(&self, job: impl FnOnce() -> Option<Done> + Send + 'static) {
        let _ = self.pictures.send(Box::new(job));
    }

    /// Wait, a little, for what has been asked of the calls' thread to be done: a film's place
    /// told to its server before the application goes, which is the last thing it does.
    pub fn finish(&self) {
        let (told, done) = channel();
        let _ = self.calls.send(Box::new(move || {
            let _ = told.send(());
            None
        }));
        let _ = done.recv_timeout(std::time::Duration::from_secs(4));
    }

    /// Something that may take a long while -- waiting for a pairing to be approved, asking
    /// the whole network who is there -- and must hold up nothing else.
    pub fn slow(&self, job: impl FnOnce() -> Option<Done> + Send + 'static) {
        let _ = self.reports.send(Box::new(job));
    }

    /// Something a server is to be told, whose answer nobody waits for.
    pub fn report(&self, job: impl FnOnce() + Send + 'static) {
        let _ = self.reports.send(Box::new(move || {
            job();
            None
        }));
    }
}
