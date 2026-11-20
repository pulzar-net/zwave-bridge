//! The Serial API link layer.
//!
//! [`SerialApi::start`] spawns a task that owns the port. Callers submit a
//! [`Request`]; the task runs one exchange at a time:
//!
//! 1. write the frame and wait up to [`Config::ack_timeout`] for ACK; on NAK,
//!    CAN or timeout retransmit after 100 ms, then 1.1 s, giving up after
//!    three transmissions;
//! 2. if a response is expected, wait for the RES frame with the same
//!    function id;
//! 3. if a callback is expected (and the response did not say the request
//!    was refused), wait for the REQ frame with the same function id whose
//!    first byte is the callback id.
//!
//! Every data frame from the controller is acknowledged at once (NAK when its
//! checksum is bad). Request frames that are not the awaited callback are
//! handed to the unsolicited channel: received commands, node information,
//! inclusion progress, SmartStart requests.

use crate::frame::{DataFrame, Frame, FrameCodec, FrameType};
use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{sleep_until, Instant};
use tokio_util::codec::Framed;
use tracing::{debug, trace, warn};

/// Link layer timing.
#[derive(Debug, Clone)]
pub struct Config {
    /// How long to wait for ACK after a frame (1600 ms in the Host API).
    pub ack_timeout: Duration,
    /// Transmissions of one frame before giving up.
    pub max_attempts: u32,
    /// How long a partially received frame may take to complete.
    pub byte_timeout: Duration,
    /// Unsolicited frames buffered for the consumer before the oldest are dropped.
    pub unsolicited_capacity: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            ack_timeout: Duration::from_millis(1600),
            max_attempts: 3,
            byte_timeout: Duration::from_millis(150),
            unsolicited_capacity: 512,
        }
    }
}

/// One exchange with the controller.
#[derive(Debug, Clone)]
pub struct Request {
    /// The frame to send.
    pub frame: DataFrame,
    /// Wait this long for a response frame; `None` when none comes.
    pub response: Option<Duration>,
    /// Wait this long for the callback with this id; `None` when none comes.
    pub callback: Option<(u8, Duration)>,
    /// Given the response, whether a callback will follow (e.g. SendData's
    /// "accepted" byte). Defaults to the first response byte being non-zero.
    pub callback_follows: fn(&DataFrame) -> bool,
}

fn first_byte_nonzero(f: &DataFrame) -> bool {
    f.payload.first().is_some_and(|&b| b != 0)
}

impl Request {
    /// A request that expects only a response.
    pub fn with_response(frame: DataFrame, timeout: Duration) -> Self {
        Request {
            frame,
            response: Some(timeout),
            callback: None,
            callback_follows: first_byte_nonzero,
        }
    }

    /// A request that expects only an ACK.
    pub fn ack_only(frame: DataFrame) -> Self {
        Request {
            frame,
            response: None,
            callback: None,
            callback_follows: first_byte_nonzero,
        }
    }

    /// A request that expects a response and then a callback.
    pub fn with_callback(
        frame: DataFrame,
        response: Duration,
        callback_id: u8,
        callback: Duration,
    ) -> Self {
        Request {
            frame,
            response: Some(response),
            callback: Some((callback_id, callback)),
            callback_follows: first_byte_nonzero,
        }
    }
}

/// What came back from an exchange.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Exchange {
    /// The response frame, if one was expected.
    pub response: Option<DataFrame>,
    /// The callback frame, if one was expected and the request was accepted.
    pub callback: Option<DataFrame>,
}

/// Link layer failures.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SerialError {
    /// No ACK after every transmission.
    #[error("no ACK from the controller after {0} attempts")]
    NoAck(u32),
    /// The controller kept answering NAK or CAN.
    #[error("controller rejected the frame {0} times")]
    Rejected(u32),
    /// No response in time.
    #[error("no response from the controller")]
    ResponseTimeout,
    /// No callback in time.
    #[error("no callback from the controller")]
    CallbackTimeout,
    /// The port closed or failed.
    #[error("serial port closed: {0}")]
    Closed(String),
}

struct Job {
    req: Request,
    reply: oneshot::Sender<Result<Exchange, SerialError>>,
}

enum Phase {
    Backoff { until: Instant },
    AwaitAck { deadline: Instant },
    AwaitResponse { deadline: Instant },
    AwaitCallback { deadline: Instant },
}

struct Current {
    job: Job,
    attempts: u32,
    nak_or_can: u32,
    phase: Phase,
    exchange: Exchange,
}

impl Current {
    fn deadline(&self) -> Instant {
        match self.phase {
            Phase::Backoff { until } => until,
            Phase::AwaitAck { deadline }
            | Phase::AwaitResponse { deadline }
            | Phase::AwaitCallback { deadline } => deadline,
        }
    }
}

/// A handle to the link layer task. Cloning is cheap; all clones share one port.
#[derive(Clone, Debug)]
pub struct SerialApi {
    jobs: mpsc::Sender<Job>,
    callback_id: Arc<AtomicU8>,
    stop: Arc<tokio::sync::Notify>,
}

impl SerialApi {
    /// Starts the link layer on `io` and returns the handle and the
    /// unsolicited frame channel.
    pub fn start<T>(io: T, config: Config) -> (SerialApi, mpsc::Receiver<DataFrame>)
    where
        T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let (jobs_tx, jobs_rx) = mpsc::channel(64);
        let (unsolicited_tx, unsolicited_rx) = mpsc::channel(config.unsolicited_capacity.max(1));
        let stop = Arc::new(tokio::sync::Notify::new());
        tokio::spawn(run(io, config, jobs_rx, unsolicited_tx, stop.clone()));
        (
            SerialApi {
                jobs: jobs_tx,
                callback_id: Arc::new(AtomicU8::new(0)),
                stop,
            },
            unsolicited_rx,
        )
    }

    /// The next callback id, 1 to 255 (0 means "no callback" on the wire).
    pub fn next_callback_id(&self) -> u8 {
        loop {
            let id = self
                .callback_id
                .fetch_add(1, Ordering::Relaxed)
                .wrapping_add(1);
            if id != 0 {
                return id;
            }
        }
    }

    /// Runs one exchange.
    pub async fn execute(&self, req: Request) -> Result<Exchange, SerialError> {
        let (tx, rx) = oneshot::channel();
        self.jobs
            .send(Job { req, reply: tx })
            .await
            .map_err(|_| SerialError::Closed("link task stopped".into()))?;
        rx.await
            .map_err(|_| SerialError::Closed("link task stopped".into()))?
    }

    /// Sends a request and returns its response frame.
    pub async fn request(
        &self,
        frame: DataFrame,
        timeout: Duration,
    ) -> Result<DataFrame, SerialError> {
        self.execute(Request::with_response(frame, timeout))
            .await?
            .response
            .ok_or(SerialError::ResponseTimeout)
    }

    /// Whether the link task is still running.
    pub fn is_open(&self) -> bool {
        !self.jobs.is_closed()
    }

    /// Stops the link task and closes the port, for every clone of this
    /// handle: the exchange in progress fails, the unsolicited channel ends,
    /// and later requests fail with [`SerialError::Closed`].
    pub fn close(&self) {
        self.stop.notify_one();
    }
}

async fn run<T>(
    io: T,
    config: Config,
    mut jobs: mpsc::Receiver<Job>,
    unsolicited: mpsc::Sender<DataFrame>,
    stop: Arc<tokio::sync::Notify>,
) where
    T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let mut framed = Framed::new(io, FrameCodec::default());
    // Resynchronize a controller that may be mid-frame from an earlier host.
    if let Err(e) = framed.send(Frame::Nak).await {
        warn!("serial write failed: {e}");
        return;
    }
    let mut current: Option<Current> = None;
    let mut partial_since: Option<Instant> = None;
    let fail_all = |current: &mut Option<Current>, e: SerialError| {
        if let Some(c) = current.take() {
            let _ = c.job.reply.send(Err(e));
        }
    };

    loop {
        let deadline = current.as_ref().map(Current::deadline);
        let byte_deadline = partial_since.map(|t| t + config.byte_timeout);
        tokio::select! {
            biased;
            () = stop.notified() => {
                fail_all(&mut current, SerialError::Closed("closed by the host".into()));
                debug!("link closed");
                return;
            }
            incoming = framed.next() => {
                let frame = match incoming {
                    Some(Ok(f)) => f,
                    Some(Err(e)) => { fail_all(&mut current, SerialError::Closed(e.to_string())); return; }
                    None => { fail_all(&mut current, SerialError::Closed("port closed".into())); return; }
                };
                partial_since = if framed.read_buffer().is_empty() { None } else { partial_since.or(Some(Instant::now())) };
                match frame {
                    Frame::Ack => {
                        if let Some(c) = current.as_mut() {
                            if matches!(c.phase, Phase::AwaitAck { .. }) {
                                after_ack(c);
                            }
                        }
                    }
                    Frame::Nak | Frame::Can => {
                        if let Some(c) = current.as_mut() {
                            if matches!(c.phase, Phase::AwaitAck { .. }) {
                                c.nak_or_can += 1;
                                debug!(attempt = c.attempts, "controller sent {frame:?}");
                                if c.attempts >= config.max_attempts {
                                    let n = c.attempts;
                                    fail_all(&mut current, SerialError::Rejected(n));
                                } else {
                                    c.phase = Phase::Backoff { until: Instant::now() + backoff(c.attempts) };
                                }
                            }
                        }
                    }
                    Frame::Invalid => {
                        debug!("frame with bad checksum, sending NAK");
                        if framed.send(Frame::Nak).await.is_err() { fail_all(&mut current, SerialError::Closed("write failed".into())); return; }
                    }
                    Frame::Data(df) => {
                        if let Err(e) = framed.send(Frame::Ack).await {
                            fail_all(&mut current, SerialError::Closed(e.to_string()));
                            return;
                        }
                        trace!(kind = ?df.kind, function = df.function, len = df.payload.len(), "rx");
                        let consumed = match current.as_mut() {
                            Some(c) => offer(c, &df),
                            None => false,
                        };
                        if !consumed {
                            if df.kind == FrameType::Request {
                                if unsolicited.try_send(df).is_err() {
                                    warn!("unsolicited frame dropped: consumer is not keeping up");
                                }
                            } else {
                                debug!(function = df.function, "unexpected response frame dropped");
                            }
                        }
                        if current.as_ref().is_some_and(|c| c.exchange_done()) {
                            let c = current.take().expect("checked");
                            let _ = c.job.reply.send(Ok(c.exchange));
                        }
                    }
                }
                if current.as_ref().is_some_and(|c| c.exchange_done()) {
                    let c = current.take().expect("checked");
                    let _ = c.job.reply.send(Ok(c.exchange));
                }
            }
            _ = async { sleep_until(byte_deadline.expect("guarded")).await }, if byte_deadline.is_some() => {
                debug!("partial frame timed out");
                let mut codec = FrameCodec::default();
                codec.reset(framed.read_buffer_mut());
                partial_since = None;
                let _ = framed.send(Frame::Nak).await;
            }
            _ = async { sleep_until(deadline.expect("guarded")).await }, if deadline.is_some() => {
                let c = current.as_mut().expect("deadline implies current");
                match c.phase {
                    Phase::Backoff { .. } => {
                        c.attempts += 1;
                        c.phase = Phase::AwaitAck { deadline: Instant::now() + config.ack_timeout };
                        if framed.send(Frame::Data(c.job.req.frame.clone())).await.is_err() {
                            fail_all(&mut current, SerialError::Closed("write failed".into()));
                            return;
                        }
                    }
                    Phase::AwaitAck { .. } => {
                        debug!(attempt = c.attempts, "no ACK");
                        if c.attempts >= config.max_attempts {
                            let n = c.attempts;
                            let e = if c.nak_or_can > 0 { SerialError::Rejected(n) } else { SerialError::NoAck(n) };
                            fail_all(&mut current, e);
                        } else {
                            c.phase = Phase::Backoff { until: Instant::now() + backoff(c.attempts) };
                        }
                    }
                    Phase::AwaitResponse { .. } => fail_all(&mut current, SerialError::ResponseTimeout),
                    Phase::AwaitCallback { .. } => fail_all(&mut current, SerialError::CallbackTimeout),
                }
            }
            job = jobs.recv(), if current.is_none() => {
                // Every handle is gone: stop, closing the port.
                let Some(job) = job else { return; };
                trace!(function = job.req.frame.function, len = job.req.frame.payload.len(), "tx");
                if let Err(e) = framed.send(Frame::Data(job.req.frame.clone())).await {
                    let _ = job.reply.send(Err(SerialError::Closed(e.to_string())));
                    return;
                }
                current = Some(Current {
                    job,
                    attempts: 1,
                    nak_or_can: 0,
                    phase: Phase::AwaitAck { deadline: Instant::now() + config.ack_timeout },
                    exchange: Exchange::default(),
                });
            }
            else => return,
        }
    }
}

fn backoff(attempts: u32) -> Duration {
    Duration::from_millis(100 + 1000 * u64::from(attempts.saturating_sub(1)))
}

fn after_ack(c: &mut Current) {
    let now = Instant::now();
    c.phase = if let Some(t) = c.job.req.response {
        Phase::AwaitResponse { deadline: now + t }
    } else if let Some((_, t)) = c.job.req.callback {
        Phase::AwaitCallback { deadline: now + t }
    } else {
        // Nothing more to wait for; exchange_done() completes it.
        Phase::AwaitCallback { deadline: now }
    };
}

impl Current {
    fn exchange_done(&self) -> bool {
        let req = &self.job.req;
        if matches!(self.phase, Phase::AwaitAck { .. } | Phase::Backoff { .. }) {
            return false;
        }
        if req.response.is_some() && self.exchange.response.is_none() {
            return false;
        }
        match (req.callback, &self.exchange.response) {
            (None, _) => true,
            (Some(_), Some(r)) if !(req.callback_follows)(r) => true,
            (Some(_), _) => self.exchange.callback.is_some(),
        }
    }
}

/// Offers a received frame to the exchange in progress; true if consumed.
fn offer(c: &mut Current, df: &DataFrame) -> bool {
    let func = c.job.req.frame.function;
    let response = c.job.req.response;
    let callback = c.job.req.callback;
    let callback_follows = c.job.req.callback_follows;
    match df.kind {
        FrameType::Response
            if df.function == func && response.is_some() && c.exchange.response.is_none() =>
        {
            // A response implies the ACK was sent even if we missed it.
            if matches!(c.phase, Phase::AwaitAck { .. } | Phase::Backoff { .. }) {
                after_ack(c);
            }
            c.exchange.response = Some(df.clone());
            if let Some((_, t)) = callback {
                if callback_follows(df) {
                    c.phase = Phase::AwaitCallback {
                        deadline: Instant::now() + t,
                    };
                }
            }
            true
        }
        FrameType::Request => match callback {
            Some((id, _))
                if df.function == func
                    && df.payload.first() == Some(&id)
                    && matches!(c.phase, Phase::AwaitCallback { .. })
                    && (response.is_none() || c.exchange.response.is_some()) =>
            {
                c.exchange.callback = Some(df.clone());
                true
            }
            _ => false,
        },
        _ => false,
    }
}
