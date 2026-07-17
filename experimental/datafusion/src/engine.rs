//! Dedicated-thread owner for the process-global Sirius context.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use arrow_array::RecordBatch;
use sirius::SiriusContext;

struct ExecuteRequest {
    plan: Vec<u8>,
    respond: Sender<Result<Vec<RecordBatch>, String>>,
}

/// Shareable handle for a Sirius context that remains on its owning thread.
#[derive(Debug)]
pub(crate) struct SiriusExecutor {
    requests: Mutex<Option<Sender<ExecuteRequest>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl SiriusExecutor {
    pub(crate) fn start(config: Option<PathBuf>) -> Result<Self, String> {
        let (request_tx, request_rx) = channel::<ExecuteRequest>();
        let (ready_tx, ready_rx) = channel::<Result<(), String>>();
        let thread = std::thread::Builder::new()
            .name("sirius-datafusion-engine".to_string())
            .spawn(move || engine_thread(config, request_rx, ready_tx))
            .map_err(|err| format!("failed to spawn Sirius engine thread: {err}"))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                requests: Mutex::new(Some(request_tx)),
                thread: Mutex::new(Some(thread)),
            }),
            Ok(Err(err)) => {
                let _ = thread.join();
                Err(err)
            }
            Err(_) => {
                let _ = thread.join();
                Err("Sirius engine thread exited during startup".to_string())
            }
        }
    }

    pub(crate) fn execute(&self, plan: Vec<u8>) -> Result<Vec<RecordBatch>, String> {
        let (respond_tx, respond_rx) = channel();
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .ok_or_else(|| "Sirius engine is shutting down".to_string())?
            .send(ExecuteRequest {
                plan,
                respond: respond_tx,
            })
            .map_err(|_| "Sirius engine thread is not running".to_string())?;
        respond_rx
            .recv()
            .map_err(|_| "Sirius engine thread dropped the response".to_string())?
    }
}

fn engine_thread(
    config: Option<PathBuf>,
    requests: Receiver<ExecuteRequest>,
    ready: Sender<Result<(), String>>,
) {
    let mut context = match config {
        Some(path) => SiriusContext::from_config_file(&path),
        None => SiriusContext::new(),
    };
    let context = match context.as_mut() {
        Ok(context) => {
            if ready.send(Ok(())).is_err() {
                return;
            }
            context
        }
        Err(err) => {
            let _ = ready.send(Err(format!("failed to start Sirius: {err}")));
            return;
        }
    };

    while let Ok(request) = requests.recv() {
        let result = context
            .execute_substrait(&request.plan)
            .map_err(|err| err.to_string());
        let _ = request.respond.send(result);
    }
}

impl Drop for SiriusExecutor {
    fn drop(&mut self) {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(thread) = self
            .thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = thread.join();
        }
    }
}
