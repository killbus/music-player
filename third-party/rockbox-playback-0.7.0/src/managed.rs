//! Host-owned streaming sessions. Cancellation is independent of the engine queue.
use crate::Metadata;
use std::io::{self, Read};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamPhase {
    Queued,
    Opening,
    Decoding,
    Cancelled,
    Failed,
    EndUnconfirmed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamReadEnd {
    Reading,
    Eof,
    Failed,
    Cancelled,
}
#[derive(Debug, Clone)]
pub struct StreamSnapshot {
    pub generation: u64,
    pub phase: StreamPhase,
    pub read_end: StreamReadEnd,
    pub cancel_requested: bool,
    /// Reader destructor (including host transport join) has returned.
    pub reader_released: bool,
    /// Decoder destructor (including codec thread join) has returned.
    pub decoder_joined: bool,
    /// Codec's own exit status, independent of transport/completion evidence.
    pub decoder_status: Option<i32>,
}
struct Inner {
    state: Mutex<StreamSnapshot>,
    wake: Box<dyn Fn() + Send + Sync>,
    // Only explicit cancellation revokes output. Normal reader EOF/drop
    // must allow already decoded PCM to drain.
    revoke_output: Box<dyn Fn() + Send + Sync>,
}
/// A single generation; never re-used for another source or retry.
#[derive(Clone)]
pub struct StreamSession(Arc<Inner>);
impl StreamSession {
    pub fn snapshot(&self) -> StreamSnapshot {
        self.0.state.lock().unwrap().clone()
    }
    pub fn generation(&self) -> u64 {
        self.0.state.lock().unwrap().generation
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.state.lock().unwrap().cancel_requested
    }
    pub fn cancel(&self) {
        {
            let mut state = self.0.state.lock().unwrap();
            state.cancel_requested = true;
            // Preserve a previously observed failure/end while still cancelling resources.
            if !matches!(
                state.phase,
                StreamPhase::Failed | StreamPhase::EndUnconfirmed
            ) {
                state.phase = StreamPhase::Cancelled;
            }
        }
        (self.0.revoke_output)();
        (self.0.wake)();
    }
    pub(crate) fn set_phase(&self, phase: StreamPhase) {
        let mut state = self.0.state.lock().unwrap();
        if !state.cancel_requested {
            state.phase = phase;
        }
    }
    pub(crate) fn decoder_joined(&self) {
        self.0.state.lock().unwrap().decoder_joined = true;
    }
    pub(crate) fn decoder_ended(&self, code: Option<i32>) {
        let mut state = self.0.state.lock().unwrap();
        state.decoder_status = code;
        if !state.cancel_requested {
            state.phase = if code != Some(0) || state.read_end == StreamReadEnd::Failed {
                StreamPhase::Failed
            } else {
                StreamPhase::EndUnconfirmed
            };
        }
    }
}
pub(crate) struct StreamInput {
    pub reader: ObservedReader,
    pub format_ext: String,
    pub metadata: Metadata,
    pub session: StreamSession,
}
impl StreamInput {
    pub fn new(
        reader: Box<dyn Read + Send>,
        format_ext: String,
        metadata: Metadata,
        generation: u64,
        wake: impl Fn() + Send + Sync + 'static,
        revoke_output: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let session = StreamSession(Arc::new(Inner {
            state: Mutex::new(StreamSnapshot {
                generation,
                phase: StreamPhase::Queued,
                read_end: StreamReadEnd::Reading,
                cancel_requested: false,
                reader_released: false,
                decoder_joined: false,
                decoder_status: None,
            }),
            wake: Box::new(wake),
            revoke_output: Box::new(revoke_output),
        }));
        Self {
            reader: ObservedReader {
                reader: Some(reader),
                session: session.clone(),
            },
            format_ext,
            metadata,
            session,
        }
    }
}
pub(crate) struct ObservedReader {
    reader: Option<Box<dyn Read + Send>>,
    session: StreamSession,
}
impl Read for ObservedReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.session.is_cancelled() {
            self.session.0.state.lock().unwrap().read_end = StreamReadEnd::Cancelled;
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "stream cancelled",
            ));
        }
        let result = self.reader.as_mut().unwrap().read(buf);
        let mut state = self.session.0.state.lock().unwrap();
        if state.cancel_requested {
            state.read_end = StreamReadEnd::Cancelled;
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "stream cancelled",
            ));
        }
        match &result {
            Ok(0) => state.read_end = StreamReadEnd::Eof,
            Err(_) => state.read_end = StreamReadEnd::Failed,
            _ => {}
        }
        result
    }
}
impl Drop for ObservedReader {
    fn drop(&mut self) {
        (self.session.0.wake)();
        drop(self.reader.take());
        self.session.0.state.lock().unwrap().reader_released = true;
    }
}
