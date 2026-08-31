//! Goal: prove `work_once`'s orchestration of `KernelPort` and
//! `AdmissionRepository` against fakes. No live kernel, no live database,
//! no live Parser.
//!
//! The two failure-semantics properties this file exists to prove:
//! - a local admission failure must never let the kernel claim be
//!   completed (`a_repository_failure_never_completes_the_kernel_claim`)
//!   -- see this repository's README, "domain commit succeeded, but
//!   kernel completion was not recorded";
//! - a stale worker that loses its claim's fencing token before it can
//!   complete must report that loss, never a success.

use std::sync::Mutex;

use infernal_pf2e_rules_simple::claims::{ClaimOutcome, CompleteOutcome, WorkClaim};
use infernal_pf2e_rules_simple::domain::{
    AdmissionError, AdmissionOutcome, AdmissionRepository, AdmittedCandidate, HeldCandidate,
    HoldOutcome, HoldResolution, ResolutionOutcome, Rule,
};
use infernal_pf2e_rules_simple::error::RulesError;
use infernal_pf2e_rules_simple::kernel_client::KernelPort;
use infernal_pf2e_rules_simple::routed_request::RoutedRequestOutcome;
use infernal_pf2e_rules_simple::routes::EligibleRoute;
use infernal_pf2e_rules_simple::{WorkOutcome, work_once};
use uuid::Uuid;

#[derive(Default)]
struct FakePort {
    routes: Vec<EligibleRoute>,
    claim_outcome: Option<ClaimOutcome>,
    request_outcome: Option<RoutedRequestOutcome>,
    complete_outcome: Option<CompleteOutcome>,
    complete_calls: Mutex<u32>,
}

impl KernelPort for FakePort {
    fn eligible_routes(&self) -> Result<Vec<EligibleRoute>, RulesError> {
        Ok(self.routes.clone())
    }

    fn propose_claim(
        &self,
        _route_id: &str,
        _lease_seconds: i64,
    ) -> Result<ClaimOutcome, RulesError> {
        Ok(self
            .claim_outcome
            .clone()
            .unwrap_or(ClaimOutcome::AlreadyClaimed))
    }

    fn routed_request(&self, _route_id: &str) -> Result<RoutedRequestOutcome, RulesError> {
        Ok(self
            .request_outcome
            .clone()
            .unwrap_or(RoutedRequestOutcome::NotFound))
    }

    fn complete_claim(
        &self,
        _claim_id: &str,
        _fencing_token: i64,
    ) -> Result<CompleteOutcome, RulesError> {
        *self.complete_calls.lock().unwrap() += 1;
        Ok(self
            .complete_outcome
            .clone()
            .unwrap_or(CompleteOutcome::NotFound))
    }
}

#[derive(Default)]
struct FakeRepository {
    result: Option<Result<AdmissionOutcome, AdmissionError>>,
}

impl AdmissionRepository for FakeRepository {
    fn admit_candidate(
        &self,
        _candidate: AdmittedCandidate,
    ) -> Result<AdmissionOutcome, AdmissionError> {
        match &self.result {
            Some(Ok(outcome)) => Ok(AdmissionOutcome {
                rule_id: outcome.rule_id,
                version: outcome.version,
                was_already_processed: outcome.was_already_processed,
            }),
            Some(Err(error)) => Err(*error),
            None => Ok(AdmissionOutcome {
                rule_id: Uuid::new_v4(),
                version: 1,
                was_already_processed: false,
            }),
        }
    }

    fn get(&self, _rule_id: Uuid, _version: Option<i64>) -> Result<Rule, AdmissionError> {
        unimplemented!("not exercised by work_once")
    }

    fn hold_candidate(
        &self,
        _candidate: AdmittedCandidate,
        _reason: String,
    ) -> Result<HoldOutcome, AdmissionError> {
        unimplemented!("not exercised by work_once -- dispatch only ever admits")
    }

    fn resolve_held(
        &self,
        _held_id: Uuid,
        _resolution: HoldResolution,
    ) -> Result<ResolutionOutcome, AdmissionError> {
        unimplemented!("not exercised by work_once -- dispatch only ever admits")
    }

    fn get_held(&self, _held_id: Uuid) -> Result<HeldCandidate, AdmissionError> {
        unimplemented!("not exercised by work_once -- dispatch only ever admits")
    }

    fn list_pending_held(&self) -> Result<Vec<HeldCandidate>, AdmissionError> {
        unimplemented!("not exercised by work_once -- dispatch only ever admits")
    }
}

fn route() -> EligibleRoute {
    EligibleRoute {
        route_id: "route-1".to_owned(),
        request_id: Uuid::new_v4().to_string(),
        subscription_id: "subscription-1".to_owned(),
        destination_service_id: "destination-1".to_owned(),
        created_at: 1,
    }
}

fn claim() -> WorkClaim {
    WorkClaim {
        claim_id: "claim-1".to_owned(),
        route_id: "route-1".to_owned(),
        worker_service_id: "destination-1".to_owned(),
        worker_instance_id: "instance-1".to_owned(),
        fencing_token: 1,
        status: "active".to_owned(),
        claimed_at: 1,
        lease_expires_at: 301,
    }
}

fn routed_request(route: &EligibleRoute, action: &str, scope: &str) -> RoutedRequestOutcome {
    RoutedRequestOutcome::Found(infernal_pf2e_rules_simple::routed_request::RoutedRequest {
        request_id: route.request_id.clone(),
        source_service_id: "source-1".to_owned(),
        action: action.to_owned(),
        scope: scope.to_owned(),
        artifact_schema_version_id: "a1".to_owned(),
        permission_policy_schema_version_id: "p1".to_owned(),
        accepted_at: 1,
    })
}

fn admit_scope() -> String {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    format!(
        "{}@pf2e-parser-0.1.0#action!0.90|{}~1~{}~p1~Stride",
        Uuid::new_v4(),
        Uuid::new_v4(),
        URL_SAFE_NO_PAD.encode([0_u8; 32])
    )
}

#[test]
fn does_nothing_when_no_route_is_eligible() {
    let port = FakePort::default();
    let repository = FakeRepository::default();

    let outcome = work_once(&port, &repository, 300).unwrap();

    assert!(matches!(outcome, WorkOutcome::NothingEligible));
}

#[test]
fn completes_a_full_admission_dispatch() {
    let route = route();
    let scope = admit_scope();
    let port = FakePort {
        routes: vec![route.clone()],
        claim_outcome: Some(ClaimOutcome::Claimed(claim())),
        request_outcome: Some(routed_request(&route, "pf2e.rules.admit", &scope)),
        complete_outcome: Some(CompleteOutcome::Completed(claim())),
        ..FakePort::default()
    };
    let repository = FakeRepository::default();

    let outcome = work_once(&port, &repository, 300).unwrap();

    assert!(matches!(outcome, WorkOutcome::Completed { .. }));
    assert_eq!(*port.complete_calls.lock().unwrap(), 1);
}

#[test]
fn a_repository_failure_never_completes_the_kernel_claim() {
    let route = route();
    let scope = admit_scope();
    let port = FakePort {
        routes: vec![route.clone()],
        claim_outcome: Some(ClaimOutcome::Claimed(claim())),
        request_outcome: Some(routed_request(&route, "pf2e.rules.admit", &scope)),
        complete_outcome: Some(CompleteOutcome::Completed(claim())),
        ..FakePort::default()
    };
    let repository = FakeRepository {
        result: Some(Err(AdmissionError::Repository)),
    };

    let result = work_once(&port, &repository, 300);

    assert!(matches!(
        result,
        Err(RulesError::Admission(AdmissionError::Repository))
    ));
    assert_eq!(
        *port.complete_calls.lock().unwrap(),
        0,
        "a failed admission must never be followed by a kernel completion call"
    );
}

#[test]
fn reports_fencing_loss_before_completion_without_erroring() {
    let route = route();
    let scope = admit_scope();
    let port = FakePort {
        routes: vec![route.clone()],
        claim_outcome: Some(ClaimOutcome::Claimed(claim())),
        request_outcome: Some(routed_request(&route, "pf2e.rules.admit", &scope)),
        complete_outcome: Some(CompleteOutcome::Fenced),
        ..FakePort::default()
    };
    let repository = FakeRepository::default();

    let outcome = work_once(&port, &repository, 300).unwrap();

    assert!(matches!(
        outcome,
        WorkOutcome::LostBeforeCompletion { route_id, claim_id }
            if route_id == "route-1" && claim_id == "claim-1"
    ));
}

#[test]
fn reports_a_lost_claim_race_without_erroring() {
    let port = FakePort {
        routes: vec![route()],
        claim_outcome: Some(ClaimOutcome::AlreadyClaimed),
        ..FakePort::default()
    };
    let repository = FakeRepository::default();

    let outcome = work_once(&port, &repository, 300).unwrap();

    assert!(matches!(outcome, WorkOutcome::ClaimLost { route_id } if route_id == "route-1"));
}

#[test]
fn a_malformed_scope_fails_the_pass_without_completing_the_claim() {
    let route = route();
    let port = FakePort {
        routes: vec![route.clone()],
        claim_outcome: Some(ClaimOutcome::Claimed(claim())),
        request_outcome: Some(routed_request(
            &route,
            "pf2e.rules.admit",
            "not-a-valid-scope",
        )),
        complete_outcome: Some(CompleteOutcome::Completed(claim())),
        ..FakePort::default()
    };
    let repository = FakeRepository::default();

    let result = work_once(&port, &repository, 300);

    assert!(matches!(result, Err(RulesError::MalformedScope(_))));
    assert_eq!(*port.complete_calls.lock().unwrap(), 0);
}
