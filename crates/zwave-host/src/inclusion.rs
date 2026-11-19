//! Inclusion and exclusion, the S2 and S0 key exchanges, and SmartStart.

use crate::controller::{Controller, RxSecurity, TxSecurity};
use crate::node::{InterviewStage, NodeRecord};
use crate::{Error, Event, NodeId, SecurityClass};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};
use zwave_cc::{security, security_2, Command};
use zwave_security::dsk::{apply_pin, Dsk};
use zwave_security::kex::{bootstrap_keys, KexFail, ECDH_CURVE25519, KEX_SCHEME_1};
use zwave_security::provisioning::{Entry, EntryStatus};
use zwave_security::s2::KeyId;
use zwave_serial::message::{add_mode, AddNodeStatus, NodeInfo, RemoveNodeStatus};
use zwave_serial::{DataFrame, FunctionId as F, Request};

/// Which keys to grant a joining S2 node.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum GrantPolicy {
    /// Ask with [`Event::GrantSecurityClasses`] and wait for
    /// [`Controller::grant_security_classes`].
    #[default]
    Ask,
    /// Grant whatever the node requests.
    Requested,
    /// Grant these (those the node also requests).
    Only(Vec<SecurityClass>),
}

/// How to include a node.
#[derive(Debug, Clone, Default)]
pub struct InclusionOptions {
    /// Keys to grant.
    pub grant: GrantPolicy,
    /// The DSK PIN, if known up front; otherwise [`Event::ValidateDsk`] asks.
    pub pin: Option<u16>,
    /// Include without any security.
    pub insecure: bool,
    /// How long to wait for a node (the configured default if `None`).
    pub timeout: Option<Duration>,
}

/// The outcome of an inclusion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InclusionResult {
    /// The new node.
    pub node: NodeId,
    /// Granted security classes, highest first.
    pub security: Vec<SecurityClass>,
    /// Why security failed, if it did (the node is then included insecurely).
    pub security_error: Option<String>,
}

#[derive(Default)]
pub(crate) struct InclusionState {
    busy: bool,
    add_tx: Option<mpsc::Sender<AddNodeStatus>>,
    remove_tx: Option<mpsc::Sender<RemoveNodeStatus>>,
    cancel: Option<oneshot::Sender<()>>,
    grant: Option<oneshot::Sender<Vec<SecurityClass>>>,
    pin: Option<oneshot::Sender<Option<u16>>>,
    listening: bool,
}

struct Busy<'a>(&'a Controller);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        let mut st = self.0.inner.inclusion.lock().expect("lock");
        st.busy = false;
        st.add_tx = None;
        st.remove_tx = None;
        st.cancel = None;
        st.grant = None;
        st.pin = None;
    }
}

const KEX_TIMEOUT: Duration = Duration::from_secs(10);

fn s2_cmd(c: impl Into<Command>) -> Vec<u8> {
    c.into().encode()
}

impl Controller {
    pub(crate) fn on_add_node_status(&self, payload: &[u8]) {
        match AddNodeStatus::parse(payload, 1) {
            Ok(s) => {
                let tx = self.inner.inclusion.lock().expect("lock").add_tx.clone();
                match tx {
                    Some(tx) => {
                        let _ = tx.try_send(s);
                    }
                    None => debug!("add node status {s:?} with no inclusion running"),
                }
            }
            Err(e) => debug!("{e}"),
        }
    }

    pub(crate) fn on_remove_node_status(&self, payload: &[u8]) {
        match RemoveNodeStatus::parse(payload, 1) {
            Ok(s) => {
                let tx = self.inner.inclusion.lock().expect("lock").remove_tx.clone();
                if let Some(tx) = tx {
                    let _ = tx.try_send(s);
                }
            }
            Err(e) => debug!("{e}"),
        }
    }

    fn take_busy(&self) -> Result<Busy<'_>, Error> {
        let mut st = self.inner.inclusion.lock().expect("lock");
        if st.busy {
            return Err(Error::Busy);
        }
        st.busy = true;
        st.listening = false;
        Ok(Busy(self))
    }

    async fn add_node(&self, payload: Vec<u8>) -> Result<(), Error> {
        self.inner
            .api
            .execute(Request::ack_only(DataFrame::request(
                F::ADD_NODE_TO_NETWORK,
                payload,
            )))
            .await?;
        Ok(())
    }

    async fn stop_add(&self) {
        if let Err(e) = self.add_node(vec![add_mode::STOP, 0]).await {
            warn!("stopping inclusion: {e}");
        }
    }

    /// Answers [`Event::GrantSecurityClasses`].
    pub fn grant_security_classes(&self, classes: Vec<SecurityClass>) -> bool {
        match self.inner.inclusion.lock().expect("lock").grant.take() {
            Some(tx) => tx.send(classes).is_ok(),
            None => false,
        }
    }

    /// Answers [`Event::ValidateDsk`] with the PIN, or `None` to abort.
    pub fn validate_dsk(&self, pin: Option<u16>) -> bool {
        match self.inner.inclusion.lock().expect("lock").pin.take() {
            Some(tx) => tx.send(pin).is_ok(),
            None => false,
        }
    }

    /// Stops a running inclusion or exclusion.
    pub fn stop_inclusion(&self) -> bool {
        match self.inner.inclusion.lock().expect("lock").cancel.take() {
            Some(tx) => tx.send(()).is_ok(),
            None => false,
        }
    }

    /// Includes a node: turns on inclusion mode, waits for a node, runs the
    /// key exchange, and starts its interview.
    pub async fn include(&self, opts: InclusionOptions) -> Result<InclusionResult, Error> {
        let busy = self.take_busy()?;
        let (tx, rx) = mpsc::channel(16);
        let (ctx, crx) = oneshot::channel();
        {
            let mut st = self.inner.inclusion.lock().expect("lock");
            st.add_tx = Some(tx);
            st.cancel = Some(ctx);
        }
        let cb = self.inner.api.next_callback_id();
        self.add_node(vec![
            add_mode::ANY | add_mode::HIGH_POWER | add_mode::NETWORK_WIDE,
            cb,
        ])
        .await?;
        self.emit(Event::InclusionStarted);
        let r = self
            .run_add(
                rx,
                crx,
                opts.timeout.unwrap_or(self.inner.cfg.inclusion_timeout),
            )
            .await;
        self.emit(Event::InclusionStopped);
        let r = match r {
            Ok((node, info)) => Ok(self.on_node_added(node, info, &opts, None).await),
            Err(e) => Err(e),
        };
        drop(busy);
        self.update_smartstart().await;
        r
    }

    /// Follows the controller's inclusion callbacks to the end.
    async fn run_add(
        &self,
        mut rx: mpsc::Receiver<AddNodeStatus>,
        mut cancel: oneshot::Receiver<()>,
        timeout: Duration,
    ) -> Result<(NodeId, NodeInfo), Error> {
        let mut deadline = tokio::time::Instant::now() + timeout;
        let mut found: Option<(NodeId, NodeInfo)> = None;
        loop {
            tokio::select! {
                s = rx.recv() => {
                    let Some(s) = s else { return Err(Error::Stopped) };
                    debug!("inclusion: {s:?}");
                    match s {
                        AddNodeStatus::Ready => {}
                        AddNodeStatus::NodeFound => deadline = tokio::time::Instant::now() + Duration::from_secs(60),
                        AddNodeStatus::AddingEndNode { node, info } | AddNodeStatus::AddingController { node, info } => found = Some((node, info)),
                        AddNodeStatus::ProtocolDone => {
                            let cb = self.inner.api.next_callback_id();
                            self.add_node(vec![add_mode::STOP, cb]).await?;
                        }
                        AddNodeStatus::Done { node } => {
                            self.stop_add().await;
                            return match found {
                                Some((n, info)) => Ok((n, info)),
                                None if node != 0 => Ok((node, NodeInfo::default())),
                                None => Err(Error::Inclusion("inclusion ended without a node".into())),
                            };
                        }
                        AddNodeStatus::Failed => {
                            self.stop_add().await;
                            return Err(Error::Inclusion("inclusion failed".into()));
                        }
                        AddNodeStatus::Other(o) => debug!("inclusion status {o:#x}"),
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    self.stop_add().await;
                    return Err(Error::Inclusion("no node joined".into()));
                }
                _ = &mut cancel => {
                    self.stop_add().await;
                    return Err(Error::Inclusion("inclusion stopped".into()));
                }
            }
        }
    }

    async fn on_node_added(
        &self,
        node: NodeId,
        info: NodeInfo,
        opts: &InclusionOptions,
        entry: Option<Entry>,
    ) -> InclusionResult {
        {
            let mut s = self.inner.store.lock().expect("store lock");
            let mut r = NodeRecord::new(node);
            for cc in &info.supported {
                r.root_mut().ccs.entry(*cc).or_default();
            }
            if !info.supported.is_empty() {
                r.stage = InterviewStage::New;
            }
            s.nodes.insert(node, r);
        }
        self.inner.info.lock().expect("lock").nodes.push(node);
        self.mark_dirty();
        self.emit(Event::NodeAdded { node });
        if let Err(e) = self.fetch_protocol_info(node).await {
            warn!(node, "protocol info: {e}");
        }
        self.assume_awake(node);
        let _a = self.activity(node);
        let supports = |cc: u16| info.supported.contains(&cc);
        let result = if opts.insecure && entry.is_none() {
            Ok(vec![])
        } else if supports(0x9f) {
            self.bootstrap_s2(node, opts, entry.as_ref()).await
        } else if supports(0x98) {
            self.bootstrap_s0(node).await
        } else {
            Ok(vec![])
        };
        let (security, security_error) = match result {
            Ok(c) => (c, None),
            Err(e) => {
                warn!(node, "security bootstrap failed: {e}");
                self.emit(Event::SecurityFailed {
                    node,
                    reason: e.to_string(),
                });
                (vec![], Some(e.to_string()))
            }
        };
        self.inner
            .s2
            .lock()
            .expect("lock")
            .set_peer_classes(node, &security);
        let dsk = entry.map(|e| e.dsk.to_string());
        let _ = self.update_node(node, |r| {
            r.security = security.clone();
            if !info.supported.is_empty() && r.stage < InterviewStage::NodeInfo {
                r.stage = InterviewStage::NodeInfo;
            }
            if dsk.is_some() {
                r.dsk = dsk;
            }
        });
        if security_error.is_none() {
            self.emit(Event::SecurityBootstrapped {
                node,
                classes: security.clone(),
            });
        }
        info!(node, ?security, "node included");
        self.spawn_interview(node);
        InclusionResult {
            node,
            security,
            security_error,
        }
    }

    async fn kex_fail(&self, node: NodeId, why: KexFail, sec: TxSecurity) {
        let _ = self
            .send_bytes(
                node,
                s2_cmd(security_2::KexFail {
                    fail_type: why as u8,
                }),
                sec,
            )
            .await;
        self.inner.s2.lock().expect("lock").end_bootstrap(node);
    }

    async fn ask<T>(&self, rx: oneshot::Receiver<T>) -> Option<T> {
        tokio::time::timeout(self.inner.cfg.user_timeout, rx)
            .await
            .ok()
            .and_then(Result::ok)
    }

    /// The controller side of the S2 key exchange.
    async fn bootstrap_s2(
        &self,
        node: NodeId,
        opts: &InclusionOptions,
        entry: Option<&Entry>,
    ) -> Result<Vec<SecurityClass>, Error> {
        let plain = TxSecurity::Plain;
        let temp = TxSecurity::S2(KeyId::Temporary);
        let (report, _) = self
            .request_secured(node, s2_cmd(security_2::KexGet {}), plain, KEX_TIMEOUT, |c, _| {
                matches!(c, Command::Security2(security_2::Command::KexReport(r)) if !r.echo)
            })
            .await?;
        let Command::Security2(security_2::Command::KexReport(report)) = report else {
            unreachable!()
        };
        if report.schemes & KEX_SCHEME_1 == 0 {
            self.kex_fail(node, KexFail::Scheme, plain).await;
            return Err(Error::Security("no supported KEX scheme".into()));
        }
        if report.curves & ECDH_CURVE25519 == 0 {
            self.kex_fail(node, KexFail::Curves, plain).await;
            return Err(Error::Security("no supported ECDH curve".into()));
        }
        let requested = SecurityClass::from_key_bits(report.keys);
        let intersect = |allowed: &[SecurityClass]| -> Vec<SecurityClass> {
            requested
                .iter()
                .copied()
                .filter(|c| allowed.contains(c))
                .collect()
        };
        let granted = match (entry, &opts.grant) {
            (Some(e), _) => intersect(&e.granted),
            (None, GrantPolicy::Requested) => requested.clone(),
            (None, GrantPolicy::Only(v)) => intersect(v),
            (None, GrantPolicy::Ask) => {
                let (tx, rx) = oneshot::channel();
                self.inner.inclusion.lock().expect("lock").grant = Some(tx);
                self.emit(Event::GrantSecurityClasses {
                    node,
                    requested: requested.clone(),
                });
                match self.ask(rx).await {
                    Some(v) => intersect(&v),
                    None => {
                        self.kex_fail(node, KexFail::Cancel, plain).await;
                        return Err(Error::Security("no answer to the key grant".into()));
                    }
                }
            }
        };
        if granted.is_empty() {
            self.kex_fail(node, KexFail::Key, plain).await;
            return Err(Error::Security("no key granted".into()));
        }
        let set = security_2::KexSet {
            request_csa: false,
            echo: false,
            scheme: KEX_SCHEME_1,
            curve: ECDH_CURVE25519,
            keys: SecurityClass::to_key_bits(&granted),
        };
        let (pk, _) = self
            .request_secured(node, s2_cmd(set.clone()), plain, KEX_TIMEOUT, |c, _| {
                matches!(c, Command::Security2(security_2::Command::PublicKeyReport(r)) if !r.including_node)
            })
            .await?;
        let Command::Security2(security_2::Command::PublicKeyReport(pk)) = pk else {
            unreachable!()
        };
        let Ok(mut node_pub) = <[u8; 32]>::try_from(pk.public_key.as_slice()) else {
            self.kex_fail(node, KexFail::Cancel, plain).await;
            return Err(Error::Security("public key is not 32 bytes".into()));
        };
        if granted.iter().any(|c| c.needs_pin()) {
            let pin = match (entry, opts.pin) {
                (Some(e), _) => {
                    if e.dsk.0[2..] != node_pub[2..16] {
                        self.kex_fail(node, KexFail::Auth, plain).await;
                        return Err(Error::Security(
                            "the node's key does not match its provisioned DSK".into(),
                        ));
                    }
                    e.dsk.pin()
                }
                (None, Some(p)) => p,
                (None, None) => {
                    let (tx, rx) = oneshot::channel();
                    self.inner.inclusion.lock().expect("lock").pin = Some(tx);
                    self.emit(Event::ValidateDsk {
                        node,
                        dsk: Dsk::from_public_key(&node_pub).masked(),
                    });
                    match self.ask(rx).await.flatten() {
                        Some(p) => p,
                        None => {
                            self.kex_fail(node, KexFail::Cancel, plain).await;
                            return Err(Error::Security("DSK not confirmed".into()));
                        }
                    }
                }
            };
            apply_pin(&mut node_pub, pin);
        } else if let Some(e) = entry {
            if e.dsk.0[..] != node_pub[..16] {
                self.kex_fail(node, KexFail::Auth, plain).await;
                return Err(Error::Security(
                    "the node's key does not match its provisioned DSK".into(),
                ));
            }
        }
        let shared = match self.inner.keypair.shared_secret(&node_pub) {
            Ok(s) => s,
            Err(_) => {
                self.kex_fail(node, KexFail::Cancel, plain).await;
                return Err(Error::Security("invalid public key".into()));
            }
        };
        let tk = bootstrap_keys(&shared, self.inner.keypair.public(), &node_pub);
        self.inner
            .s2
            .lock()
            .expect("lock")
            .begin_bootstrap(node, tk);
        let result = self.exchange_keys(node, &set, &report, &granted).await;
        match result {
            Ok(()) => {
                let mut s2 = self.inner.s2.lock().expect("lock");
                s2.end_bootstrap(node);
                s2.set_peer_classes(node, &granted);
                Ok(granted)
            }
            Err((why, e)) => {
                self.kex_fail(node, why, temp).await;
                Err(e)
            }
        }
    }

    async fn exchange_keys(
        &self,
        node: NodeId,
        set: &security_2::KexSet,
        report: &security_2::KexReport,
        granted: &[SecurityClass],
    ) -> Result<(), (KexFail, Error)> {
        let temp = TxSecurity::S2(KeyId::Temporary);
        let is_temp = |s: Option<RxSecurity>| s == Some(RxSecurity::S2(KeyId::Temporary));
        let own_pub = *self.inner.keypair.public();
        let (echo, _) = self
            .request_secured(
                node,
                s2_cmd(security_2::PublicKeyReport {
                    including_node: true,
                    public_key: own_pub.to_vec(),
                }),
                TxSecurity::Plain,
                KEX_TIMEOUT,
                move |c, s| {
                    is_temp(s)
                        && matches!(c, Command::Security2(security_2::Command::KexSet(k)) if k.echo)
                },
            )
            .await
            .map_err(|e| (KexFail::Decrypt, e))?;
        let Command::Security2(security_2::Command::KexSet(echo)) = echo else {
            unreachable!()
        };
        if echo.scheme != set.scheme || echo.curve != set.curve || echo.keys != set.keys {
            return Err((
                KexFail::Auth,
                Error::Security("the echoed KEX Set differs".into()),
            ));
        }
        let mut next = self.wait_for(node, false, move |i| {
            matches!(i, crate::controller::Inbound::Command { command: Command::Security2(security_2::Command::NetworkKeyGet(_)), security, .. } if is_temp(*security))
        });
        self.send_bytes(
            node,
            s2_cmd(security_2::KexReport {
                request_csa: report.request_csa,
                echo: true,
                schemes: report.schemes,
                curves: report.curves,
                keys: report.keys,
            }),
            temp,
        )
        .await
        .map_err(|e| (KexFail::Cancel, e))?;
        let mut sent: Vec<SecurityClass> = Vec::new();
        loop {
            let got = tokio::time::timeout(KEX_TIMEOUT, next.1).await;
            drop(next.0);
            let Ok(Ok(crate::controller::Inbound::Command { command, .. })) = got else {
                return Err((KexFail::Cancel, Error::Timeout(node)));
            };
            match command {
                Command::Security2(security_2::Command::NetworkKeyGet(g)) => {
                    let Some(class) = SecurityClass::from_key_bit(g.requested_key)
                        .filter(|c| granted.contains(c) && !sent.contains(c))
                    else {
                        return Err((
                            KexFail::KeyGet,
                            Error::Security(format!(
                                "the node asked for key {:#04x}",
                                g.requested_key
                            )),
                        ));
                    };
                    let mut classes = sent.clone();
                    classes.push(class);
                    self.inner
                        .s2
                        .lock()
                        .expect("lock")
                        .set_peer_classes(node, &classes);
                    let (verify, _) = self
                        .request_secured(
                            node,
                            s2_cmd(security_2::NetworkKeyReport {
                                granted_key: class.key_bit(),
                                network_key: self.inner.cfg.keys.key(class).to_vec(),
                            }),
                            temp,
                            KEX_TIMEOUT,
                            move |c, s| {
                                s == Some(RxSecurity::S2(KeyId::Class(class)))
                                    && matches!(
                                        c,
                                        Command::Security2(security_2::Command::NetworkKeyVerify(
                                            _
                                        ))
                                    )
                            },
                        )
                        .await
                        .map_err(|e| (KexFail::KeyVerify, e))?;
                    debug!(node, %class, "key verified: {}", verify.name());
                    sent.push(class);
                    next = self.wait_for(node, false, move |i| {
                        matches!(i, crate::controller::Inbound::Command { command: Command::Security2(security_2::Command::NetworkKeyGet(_) | security_2::Command::TransferEnd(_)), security, .. } if is_temp(*security))
                    });
                    self.send_bytes(
                        node,
                        s2_cmd(security_2::TransferEnd {
                            key_verified: true,
                            key_request_complete: false,
                        }),
                        temp,
                    )
                    .await
                    .map_err(|e| (KexFail::Cancel, e))?;
                }
                Command::Security2(security_2::Command::TransferEnd(t))
                    if t.key_request_complete =>
                {
                    if sent.len() != granted.len() {
                        return Err((
                            KexFail::Key,
                            Error::Security("the node did not take every granted key".into()),
                        ));
                    }
                    return Ok(());
                }
                other => {
                    return Err((
                        KexFail::Cancel,
                        Error::Security(format!("unexpected {}", other.name())),
                    ))
                }
            }
        }
    }

    /// The controller side of the S0 key exchange.
    async fn bootstrap_s0(&self, node: NodeId) -> Result<Vec<SecurityClass>, Error> {
        self.request_secured(
            node,
            Command::from(security::SchemeGet { schemes: 0 }).encode(),
            TxSecurity::Plain,
            KEX_TIMEOUT,
            |c, _| matches!(c, Command::Security(security::Command::SchemeReport(_))),
        )
        .await?;
        let set = Command::from(security::NetworkKeySet {
            network_key: self.inner.cfg.keys.s0.to_vec(),
        })
        .encode();
        self.request_secured(
            node,
            set,
            TxSecurity::S0 { temp: true },
            KEX_TIMEOUT,
            |c, s| {
                s == Some(RxSecurity::S0)
                    && matches!(c, Command::Security(security::Command::NetworkKeyVerify(_)))
            },
        )
        .await?;
        Ok(vec![SecurityClass::S0Legacy])
    }

    /// Excludes a node: turns on exclusion mode and waits for one.
    /// Returns the node id it had in this network, if any.
    pub async fn exclude(&self, timeout: Option<Duration>) -> Result<Option<NodeId>, Error> {
        let busy = self.take_busy()?;
        let (tx, mut rx) = mpsc::channel(16);
        let (ctx, mut crx) = oneshot::channel();
        {
            let mut st = self.inner.inclusion.lock().expect("lock");
            st.remove_tx = Some(tx);
            st.cancel = Some(ctx);
        }
        let api = &self.inner.api;
        let remove = |p: Vec<u8>| {
            api.execute(Request::ack_only(DataFrame::request(
                F::REMOVE_NODE_FROM_NETWORK,
                p,
            )))
        };
        let cb = api.next_callback_id();
        remove(vec![
            add_mode::ANY | add_mode::HIGH_POWER | add_mode::NETWORK_WIDE,
            cb,
        ])
        .await?;
        self.emit(Event::ExclusionStarted);
        let deadline =
            tokio::time::Instant::now() + timeout.unwrap_or(self.inner.cfg.inclusion_timeout);
        let mut removed = None;
        let result = loop {
            tokio::select! {
                s = rx.recv() => match s {
                    Some(RemoveNodeStatus::RemovingEndNode { node } | RemoveNodeStatus::RemovingController { node }) => removed = Some(node),
                    Some(RemoveNodeStatus::Done { node }) => break Ok(removed.or(Some(node)).filter(|&n| n != 0)),
                    Some(RemoveNodeStatus::Failed) => break Err(Error::Inclusion("exclusion failed".into())),
                    Some(_) => {}
                    None => break Err(Error::Stopped),
                },
                _ = tokio::time::sleep_until(deadline) => break Err(Error::Inclusion("no node left".into())),
                _ = &mut crx => break Err(Error::Inclusion("exclusion stopped".into())),
            }
        };
        let _ = remove(vec![add_mode::STOP, 0]).await;
        self.emit(Event::ExclusionStopped);
        if let Ok(Some(n)) = result {
            self.forget_node(n);
        }
        drop(busy);
        self.update_smartstart().await;
        result
    }

    /// Removes a node the controller considers failed (it stopped answering).
    pub async fn remove_failed_node(&self, node: NodeId) -> Result<(), Error> {
        let cb = self.inner.api.next_callback_id();
        let req = Request::with_callback(
            DataFrame::request(F::REMOVE_FAILED_NODE, vec![node as u8, cb]),
            Duration::from_secs(2),
            cb,
            Duration::from_secs(30),
        );
        let mut req = req;
        req.callback_follows = |r| r.payload.first() == Some(&0);
        let ex = self.inner.api.execute(req).await?;
        match ex.callback.as_ref().and_then(|c| c.payload.get(1)) {
            Some(0x01) => {
                self.forget_node(node);
                Ok(())
            }
            _ => Err(Error::Controller(format!(
                "node {node} was not removed (it may still be answering)"
            ))),
        }
    }

    // ------------------------------------------------------------- SmartStart

    /// Adds or replaces a provisioning list entry (from a scanned QR code).
    pub async fn provision(&self, entry: Entry) -> Result<(), Error> {
        {
            let mut s = self.inner.store.lock().expect("store lock");
            if !s.provisioning.upsert(entry) {
                return Err(Error::Store("the provisioning list is full".into()));
            }
        }
        self.provisioning_changed().await;
        Ok(())
    }

    /// Removes a provisioning list entry.
    pub async fn unprovision(&self, dsk: &Dsk) -> Option<Entry> {
        let e = self
            .inner
            .store
            .lock()
            .expect("store lock")
            .provisioning
            .remove(dsk);
        self.provisioning_changed().await;
        e
    }

    /// The provisioning list.
    pub fn provisioning_list(&self) -> Vec<Entry> {
        self.inner
            .store
            .lock()
            .expect("store lock")
            .provisioning
            .entries()
            .to_vec()
    }

    async fn provisioning_changed(&self) {
        self.mark_dirty();
        self.emit(Event::ProvisioningChanged {
            entries: self.provisioning_list(),
        });
        self.update_smartstart().await;
    }

    /// Listens for SmartStart requests while the provisioning list has active entries.
    pub(crate) async fn update_smartstart(&self) {
        let want = self
            .inner
            .store
            .lock()
            .expect("store lock")
            .provisioning
            .has_active();
        let change = {
            let st = self.inner.inclusion.lock().expect("lock");
            !st.busy && st.listening != want
        };
        if !change {
            return;
        }
        let payload = if want {
            vec![
                add_mode::SMART_START_LISTEN | add_mode::HIGH_POWER | add_mode::NETWORK_WIDE,
                0,
            ]
        } else {
            vec![add_mode::STOP, 0]
        };
        match self.add_node(payload).await {
            Ok(()) => self.inner.inclusion.lock().expect("lock").listening = want,
            Err(e) => warn!("SmartStart listen mode: {e}"),
        }
    }

    pub(crate) fn on_smartstart_request(&self, nwi: [u8; 4]) {
        let entry = self
            .inner
            .store
            .lock()
            .expect("store lock")
            .provisioning
            .match_nwi_home_id(&nwi)
            .cloned();
        let Some(entry) = entry else {
            debug!("SmartStart request from a node not on the provisioning list");
            return;
        };
        if entry.status != EntryStatus::Active || entry.node_id.is_some() {
            return;
        }
        let c = self.clone();
        tokio::spawn(async move {
            match c.include_smartstart(entry).await {
                Ok(r) => info!(node = r.node, "SmartStart inclusion done"),
                Err(e) => warn!("SmartStart inclusion: {e}"),
            }
        });
    }

    async fn include_smartstart(&self, entry: Entry) -> Result<InclusionResult, Error> {
        let busy = self.take_busy()?;
        let (tx, rx) = mpsc::channel(16);
        let (ctx, crx) = oneshot::channel();
        {
            let mut st = self.inner.inclusion.lock().expect("lock");
            st.add_tx = Some(tx);
            st.cancel = Some(ctx);
        }
        let cb = self.inner.api.next_callback_id();
        let mut p = vec![
            add_mode::SMART_START_DSK | add_mode::HIGH_POWER | add_mode::NETWORK_WIDE,
            cb,
        ];
        p.extend_from_slice(&entry.dsk.nwi_home_id());
        p.extend_from_slice(&entry.dsk.auth_home_id());
        self.add_node(p).await?;
        self.emit(Event::InclusionStarted);
        let r = self
            .run_add(rx, crx, self.inner.cfg.inclusion_timeout)
            .await;
        self.emit(Event::InclusionStopped);
        let r = match r {
            Ok((node, info)) => {
                let res = self
                    .on_node_added(
                        node,
                        info,
                        &InclusionOptions::default(),
                        Some(entry.clone()),
                    )
                    .await;
                if let Some(e) = self
                    .inner
                    .store
                    .lock()
                    .expect("store lock")
                    .provisioning
                    .get_mut(&entry.dsk)
                {
                    e.node_id = Some(node);
                }
                self.mark_dirty();
                Ok(res)
            }
            Err(e) => Err(e),
        };
        drop(busy);
        self.update_smartstart().await;
        r
    }
}
