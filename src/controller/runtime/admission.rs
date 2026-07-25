use crate::controller::NextAction;
use anyhow::{bail, Result};
use std::collections::HashSet;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

const ADMISSION_RESPONSE_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub(crate) enum RouteAdmissionResponse {
    Admitted(super::node_capacity::NodeCapacityLease),
    Deferred(String),
    Shutdown,
}

#[derive(Debug)]
pub(crate) struct RouteAdmissionRequest {
    pub(crate) sequence: usize,
    pub(crate) action: NextAction,
    pub(crate) response: Sender<RouteAdmissionResponse>,
}

#[derive(Debug)]
pub(crate) enum AdmissionMessage {
    Request(RouteAdmissionRequest),
    Released(usize),
}

/// The route side of the controller's two-phase admission handshake.
///
/// A dispatch first reserves its selected backend/model route, then calls
/// `wait_for_node`. The controller samples and reserves node capacity only
/// after receiving that request. The route guard remains owned by the
/// dispatch, so any rejection or shutdown drops it before the backend starts.
#[derive(Clone, Debug)]
pub struct RouteNodeAdmission {
    sequence: usize,
    action: NextAction,
    requests: Sender<AdmissionMessage>,
}

impl RouteNodeAdmission {
    pub(crate) fn new(
        sequence: usize,
        action: NextAction,
        requests: Sender<AdmissionMessage>,
    ) -> Self {
        Self {
            sequence,
            action,
            requests,
        }
    }

    pub(crate) fn wait_for_node(&self) -> Result<WorkerNodeLease> {
        if crate::runner::shutdown_requested() {
            bail!("shutdown requested before node admission");
        }
        let (response, responses) = channel();
        self.requests
            .send(AdmissionMessage::Request(RouteAdmissionRequest {
                sequence: self.sequence,
                action: self.action.clone(),
                response,
            }))
            .map_err(|_| anyhow::anyhow!("controller closed node-admission requests"))?;

        loop {
            match responses.recv_timeout(ADMISSION_RESPONSE_POLL_INTERVAL) {
                Ok(RouteAdmissionResponse::Admitted(lease)) => {
                    return Ok(WorkerNodeLease {
                        _lease: lease,
                        sequence: self.sequence,
                        requests: self.requests.clone(),
                    });
                }
                Ok(RouteAdmissionResponse::Deferred(reason)) => {
                    return Err(NodeAdmissionDeferred(reason).into());
                }
                Ok(RouteAdmissionResponse::Shutdown) => {
                    bail!("shutdown requested while waiting for node admission");
                }
                Err(RecvTimeoutError::Timeout) if crate::runner::shutdown_requested() => {
                    bail!("shutdown requested while waiting for node admission");
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    bail!("controller closed node-admission response")
                }
            }
        }
    }

    #[cfg(test)]
    fn test_with_response(action: NextAction, response: RouteAdmissionResponse) -> Self {
        let (requests, receiver) = request_channel();
        std::thread::spawn(move || {
            let Ok(AdmissionMessage::Request(request)) = receiver.recv() else {
                return;
            };
            let _ = request.response.send(response);
        });
        Self::new(1, action, requests)
    }

    #[cfg(test)]
    pub(crate) fn test_admitted(
        action: NextAction,
        lease: super::node_capacity::NodeCapacityLease,
    ) -> Self {
        Self::test_with_response(action, RouteAdmissionResponse::Admitted(lease))
    }

    #[cfg(test)]
    pub(crate) fn test_deferred(action: NextAction, reason: impl Into<String>) -> Self {
        Self::test_with_response(action, RouteAdmissionResponse::Deferred(reason.into()))
    }
}

#[derive(Debug)]
pub(crate) struct WorkerNodeLease {
    _lease: super::node_capacity::NodeCapacityLease,
    sequence: usize,
    requests: Sender<AdmissionMessage>,
}

impl Drop for WorkerNodeLease {
    fn drop(&mut self) {
        let _ = self
            .requests
            .send(AdmissionMessage::Released(self.sequence));
    }
}

#[derive(Debug)]
pub(crate) struct NodeAdmissionDeferred(pub(crate) String);

impl std::fmt::Display for NodeAdmissionDeferred {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "node admission deferred: {}", self.0)
    }
}

impl std::error::Error for NodeAdmissionDeferred {}

pub(crate) fn request_channel() -> (Sender<AdmissionMessage>, Receiver<AdmissionMessage>) {
    channel()
}

pub(crate) enum PollOutcome {
    Empty,
    Admitted,
    Deferred { action: NextAction, reason: String },
    Shutdown,
    Released,
    Disconnected,
}

pub(crate) struct Coordinator {
    requests: Receiver<AdmissionMessage>,
    node_admitted_sequences: HashSet<usize>,
}

impl Coordinator {
    pub(crate) fn new(requests: Receiver<AdmissionMessage>) -> Self {
        Self {
            requests,
            node_admitted_sequences: HashSet::new(),
        }
    }

    pub(crate) fn active_node_workers(&self) -> usize {
        self.node_admitted_sequences.len()
    }

    pub(crate) fn register_lifecycle_worker(&mut self, sequence: usize) {
        self.node_admitted_sequences.insert(sequence);
    }

    pub(crate) fn complete_worker(&mut self, sequence: usize) {
        self.node_admitted_sequences.remove(&sequence);
    }

    pub(crate) fn poll(&mut self) -> PollOutcome {
        let request = match self.requests.try_recv() {
            Ok(AdmissionMessage::Request(request)) => request,
            Ok(AdmissionMessage::Released(sequence)) => {
                self.complete_worker(sequence);
                return PollOutcome::Released;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => return PollOutcome::Empty,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                return PollOutcome::Disconnected;
            }
        };
        let sequence = request.sequence;
        let action = request.action.clone();
        if crate::runner::shutdown_requested() {
            let _ = request.response.send(RouteAdmissionResponse::Shutdown);
            return PollOutcome::Shutdown;
        }

        match super::node_capacity::try_acquire(&action, self.active_node_workers()) {
            Ok(super::node_capacity::LiveAdmission::Admit(lease)) => {
                self.node_admitted_sequences.insert(sequence);
                if request
                    .response
                    .send(RouteAdmissionResponse::Admitted(lease))
                    .is_err()
                {
                    self.complete_worker(sequence);
                }
                PollOutcome::Admitted
            }
            Ok(super::node_capacity::LiveAdmission::Defer(reason)) => {
                let _ = request
                    .response
                    .send(RouteAdmissionResponse::Deferred(reason.clone()));
                PollOutcome::Deferred { action, reason }
            }
            Err(error) => {
                let reason = format!("node pressure unavailable: {error}");
                let _ = request
                    .response
                    .send(RouteAdmissionResponse::Deferred(reason.clone()));
                PollOutcome::Deferred { action, reason }
            }
        }
    }
}

pub(crate) fn service_pending_request(
    coordinator: &mut Coordinator,
    active_workers: usize,
    parallel_limit: usize,
    reprobe: &mut super::node_reprobe::NodeCapacityReprobe,
) -> Result<bool> {
    match coordinator.poll() {
        PollOutcome::Admitted => {
            reprobe.clear();
            Ok(true)
        }
        PollOutcome::Deferred { action, reason } => {
            eprintln!(
                "gah loop: route is ready but node admission deferred at {active_workers}/{parallel_limit}: {reason}"
            );
            if active_workers > 0 {
                reprobe.schedule(action);
            }
            Ok(true)
        }
        PollOutcome::Shutdown | PollOutcome::Released => Ok(true),
        PollOutcome::Empty => Ok(false),
        PollOutcome::Disconnected if active_workers > 0 => {
            bail!("parallel route-admission channel closed unexpectedly")
        }
        PollOutcome::Disconnected => Ok(false),
    }
}

pub(crate) fn recv_worker_done<T>(receiver: &Receiver<T>) -> Result<Option<T>> {
    match receiver.recv_timeout(Duration::from_millis(250)) {
        Ok(result) => Ok(Some(result)),
        Err(RecvTimeoutError::Timeout) if crate::runner::shutdown_requested() => Ok(None),
        Err(RecvTimeoutError::Timeout) => Ok(None),
        Err(RecvTimeoutError::Disconnected) => {
            bail!("parallel GAH worker channel closed unexpectedly")
        }
    }
}

pub(crate) fn action_needs_handshake(action: &NextAction) -> bool {
    matches!(
        action,
        NextAction::DispatchTicket { .. }
            | NextAction::Retry { .. }
            | NextAction::Escalate { .. }
            | NextAction::FixMr { .. }
            | NextAction::ReviewMr { .. }
            | NextAction::DecomposeIssue { .. }
    )
}

pub(crate) fn allow_bounded_node_alternative(
    outcome: &str,
    attempts_remaining: &mut usize,
    fill_attempts_remaining: &mut usize,
    refill_suppressed: bool,
) -> bool {
    if refill_suppressed
        || *attempts_remaining == 0
        || !outcome.starts_with("Deferred ")
        || !outcome.contains("because node capacity is busy")
    {
        return false;
    }
    *attempts_remaining -= 1;
    // `executed_work_ids` prevents reselecting the just-deferred action. This
    // cap lets a lighter queued action run without spinning an all-heavy queue.
    *fill_attempts_remaining = (*fill_attempts_remaining).max(1);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn implementation() -> NextAction {
        NextAction::DispatchTicket {
            ticket_path: "ticket.md".into(),
            work_id: Some("#781".into()),
            recommended_backend: None,
            recommended_model: None,
            reason: "queued".into(),
        }
    }

    fn admission_request(message: AdmissionMessage) -> RouteAdmissionRequest {
        match message {
            AdmissionMessage::Request(request) => request,
            AdmissionMessage::Released(sequence) => {
                panic!("unexpected release for sequence {sequence}")
            }
        }
    }

    #[test]
    fn route_blocked_worker_requests_no_node_capacity() {
        let (requests, receiver) = request_channel();
        let handshake = RouteNodeAdmission::new(1, implementation(), requests);
        let route_available = Arc::new(AtomicBool::new(false));
        let worker_route_available = Arc::clone(&route_available);
        let worker = std::thread::spawn(move || {
            while !worker_route_available.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            handshake.wait_for_node()
        });

        assert!(
            receiver.try_recv().is_err(),
            "a worker still waiting for its route must not request a node lease"
        );
        route_available.store(true, Ordering::Release);
        let request = admission_request(
            receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("route-ready worker should request node admission"),
        );
        let temp = tempfile::tempdir().unwrap();
        let lease = super::super::node_capacity::test_lease(temp.path())
            .unwrap()
            .0;
        request
            .response
            .send(RouteAdmissionResponse::Admitted(lease))
            .unwrap();
        worker.join().unwrap().unwrap();
    }

    #[test]
    fn admitted_worker_keeps_node_lease_after_scheduler_side_exits() {
        let temp = tempfile::tempdir().unwrap();
        let (lease, lease_path) = super::super::node_capacity::test_lease(temp.path()).unwrap();
        let (requests, receiver) = request_channel();
        let handshake = RouteNodeAdmission::new(4, implementation(), requests);
        let (backend_started, started) = channel();
        let (release, released) = channel();
        let worker = std::thread::spawn(move || {
            let _node_lease = handshake.wait_for_node()?;
            backend_started.send(()).unwrap();
            released.recv().unwrap();
            Ok::<_, anyhow::Error>(())
        });

        let request = admission_request(receiver.recv_timeout(Duration::from_secs(1)).unwrap());
        request
            .response
            .send(RouteAdmissionResponse::Admitted(lease))
            .unwrap();
        started.recv_timeout(Duration::from_secs(1)).unwrap();

        // Model an early scheduler error: its receiver and request-side state
        // disappear while the scoped backend thread remains blocked.
        drop(receiver);
        assert!(
            lease_path.exists(),
            "worker, not scheduler, must own the live node lease"
        );

        release.send(()).unwrap();
        worker.join().unwrap().unwrap();
        assert!(
            !lease_path.exists(),
            "lease must release only after the backend worker exits"
        );
    }

    #[test]
    fn node_rejection_releases_route_owner_without_backend_start() {
        struct RouteProbe(Arc<AtomicBool>);
        impl Drop for RouteProbe {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }

        let (requests, receiver) = request_channel();
        let handshake = RouteNodeAdmission::new(2, implementation(), requests);
        let route_released = Arc::new(AtomicBool::new(false));
        let released = Arc::clone(&route_released);
        let backend_started = Arc::new(AtomicBool::new(false));
        let started = Arc::clone(&backend_started);
        let worker = std::thread::spawn(move || {
            let _route = RouteProbe(released);
            handshake.wait_for_node()?;
            started.store(true, Ordering::Release);
            Ok::<_, anyhow::Error>(())
        });

        let request = admission_request(receiver.recv_timeout(Duration::from_secs(1)).unwrap());
        request
            .response
            .send(RouteAdmissionResponse::Deferred("memory reserve".into()))
            .unwrap();
        let error = worker.join().unwrap().unwrap_err();
        assert!(error.downcast_ref::<NodeAdmissionDeferred>().is_some());
        assert!(route_released.load(Ordering::Acquire));
        assert!(!backend_started.load(Ordering::Acquire));
    }

    #[test]
    fn shutdown_releases_route_owner_promptly() {
        struct RouteProbe(Arc<AtomicBool>);
        impl Drop for RouteProbe {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }

        let (requests, receiver) = request_channel();
        let handshake = RouteNodeAdmission::new(3, implementation(), requests);
        let route_released = Arc::new(AtomicBool::new(false));
        let released = Arc::clone(&route_released);
        let worker = std::thread::spawn(move || {
            let _route = RouteProbe(released);
            handshake.wait_for_node()
        });
        let request = admission_request(receiver.recv_timeout(Duration::from_secs(1)).unwrap());
        request
            .response
            .send(RouteAdmissionResponse::Shutdown)
            .unwrap();
        let error = worker.join().unwrap().unwrap_err();
        assert!(error.to_string().contains("shutdown requested"));
        assert!(route_released.load(Ordering::Acquire));
    }

    #[test]
    fn node_pressure_alternatives_are_bounded() {
        let mut alternatives = 1;
        let mut fill = 0;
        assert!(allow_bounded_node_alternative(
            "Deferred dispatch_ticket because node capacity is busy; no backend launched",
            &mut alternatives,
            &mut fill,
            false,
        ));
        assert_eq!(fill, 1);
        fill = 0;
        assert!(!allow_bounded_node_alternative(
            "Deferred retry because node capacity is busy; no backend launched",
            &mut alternatives,
            &mut fill,
            false,
        ));
    }
}
