//! One bounded inference job, with no frame backlog. Capture and rendering stay
//! on their own thread; a stalled model is detected by the worker watchdog.
use crate::{
    capture::Monitor,
    detection::{Detector, Face, Image, Region},
};
use anyhow::{Result, anyhow};
use std::{
    sync::mpsc::{self, Receiver, SyncSender},
    time::Instant,
};

pub struct Job {
    pub monitor: Monitor,
    pub generation: u64,
    pub pixels: Vec<u8>,
    pub region: Region,
    pub minimum: f32,
    pub tile: Option<(usize, Option<u64>)>,
    pub threshold: f32,
    pub mode: u32,
    pub captured: Instant,
}

pub struct Completed {
    pub job: Job,
    pub faces: Vec<Face>,
}

pub struct Analyzer {
    sender: SyncSender<Job>,
    receiver: Receiver<Result<Completed, String>>,
}

impl Analyzer {
    pub fn new() -> Result<Self> {
        let (sender, jobs) = mpsc::sync_channel::<Job>(1);
        let (results, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("Pyra inference".into())
            .spawn(move || {
                let detector = Detector::new(&crate::native::runtime());
                let mut detector = match detector {
                    Ok(detector) => detector,
                    Err(error) => {
                        let _ = results.send(Err(format!("initialize detection: {error:#}")));
                        return;
                    }
                };
                while let Ok(job) = jobs.recv() {
                    detector.set_mode(job.mode);
                    let image = Image {
                        pixels: &job.pixels,
                        width: job.monitor.w,
                        height: job.monitor.h,
                    };
                    match detector.detect(&image, job.region, job.threshold) {
                        Ok(faces) => {
                            if results.send(Ok(Completed { job, faces })).is_err() {
                                break;
                            }
                        }
                        Err(error) => {
                            let _ = results.send(Err(format!("detection failed: {error:#}")));
                            break;
                        }
                    }
                }
            })?;
        Ok(Self { sender, receiver })
    }

    pub fn submit(&self, job: Job) -> Result<()> {
        self.sender
            .try_send(job)
            .map_err(|_| anyhow!("inference queue unavailable"))
    }

    pub fn poll(&self) -> Result<Option<Completed>> {
        match self.receiver.try_recv() {
            Ok(result) => result.map(Some).map_err(|error| anyhow!(error)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(anyhow!("inference thread exited")),
        }
    }
}
