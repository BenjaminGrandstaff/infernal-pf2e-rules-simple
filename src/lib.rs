//! PF2e Rules Service: owns authoritative structured Pathfinder 2e rule
//! data, versions, relationships, and search. It does not parse source
//! documents -- see `infernal-pf2e-parser-simple` for that, and
//! `docs/architecture/decisions/0001-separate-pf2e-parsing-from-pf2e-
//! rule-authority.md` for why they are split.
//!
//! All communication with other services is mediated through Infernal
//! Law: this service claims its own `pf2e.rules.admit` work directly and
//! never calls the Parser, or reads its database, in-process.

pub mod claims;
pub mod database;
pub mod dispatch;
pub mod domain;
pub mod error;
pub mod health;
pub mod instance_lease;
pub mod kernel_client;
pub mod postgres_rules_repository;
pub mod routed_request;
pub mod routes;
pub mod subscriptions;

use std::env;
use std::time::Duration;

use infernal_client::ClientCredential;
use uuid::Uuid;

use crate::claims::ClaimOutcome;
use crate::database::Database;
use crate::domain::AdmissionRepository;
use crate::error::RulesError;
use crate::instance_lease::RENEWAL_MARGIN_SECONDS;
use crate::kernel_client::{KernelClient, KernelPort};
use crate::postgres_rules_repository::PostgresRulesRepository;
use crate::routed_request::RoutedRequestOutcome;

const KERNEL_AUTHORITY_ENV: &str = "KERNEL_AUTHORITY";
const RULES_SERVICE_ID_ENV: &str = "RULES_SERVICE_ID";
const CLAIM_LEASE_SECONDS_ENV: &str = "CLAIM_LEASE_SECONDS";
const POLL_INTERVAL_SECONDS_ENV: &str = "POLL_INTERVAL_SECONDS";
const KERNEL_CA_CERT_PATH_ENV: &str = "KERNEL_CA_CERT_PATH";
const ENROLLMENT_CHALLENGE_ENV: &str = "ENROLLMENT_CHALLENGE";
const SERVICE_ENDPOINT_ENV: &str = "SERVICE_ENDPOINT";
const POD_UID_ENV: &str = "POD_UID";
const WORKLOAD_TOKEN_PATH_ENV: &str = "WORKLOAD_TOKEN_PATH";
const DEFAULT_WORKLOAD_TOKEN_PATH: &str = "/var/run/secrets/infernal-law-enrollment/token";
const DEFAULT_LEASE_SECONDS: i64 = 300;
const DEFAULT_POLL_INTERVAL_SECONDS: u64 = 5;

pub struct Config {
    pub client: KernelClient,
    pub repository: PostgresRulesRepository,
    pub lease_seconds: i64,
    pub poll_interval: Duration,
    pub instance_lease: Option<InstanceLease>,
}

#[derive(Clone, Copy, Debug)]
pub struct InstanceLease {
    pub revision: i64,
    pub expires_at: i64,
}

impl Config {
    pub fn from_env() -> Result<Self, RulesError> {
        let authority = env::var(KERNEL_AUTHORITY_ENV)
            .map_err(|_| RulesError::MissingEnv(KERNEL_AUTHORITY_ENV))?;
        let service_id: Uuid = env::var(RULES_SERVICE_ID_ENV)
            .map_err(|_| RulesError::MissingEnv(RULES_SERVICE_ID_ENV))?
            .parse()
            .map_err(|_| RulesError::InvalidServiceId)?;
        let lease_seconds = env::var(CLAIM_LEASE_SECONDS_ENV)
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_LEASE_SECONDS);
        let poll_interval_seconds = env::var(POLL_INTERVAL_SECONDS_ENV)
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_POLL_INTERVAL_SECONDS);
        let credential = ClientCredential::generate(service_id);
        let client = match env::var(KERNEL_CA_CERT_PATH_ENV) {
            Ok(path) => {
                let pem = std::fs::read(&path).map_err(RulesError::CaCertificateUnreadable)?;
                KernelClient::with_extra_root_certificate(credential, authority, &pem)?
            }
            Err(_) => KernelClient::new(credential, authority)?,
        };
        let mut instance_lease = None;
        // Enrollment is driven by SERVICE_ENDPOINT and POD_UID rather than
        // by ENROLLMENT_CHALLENGE: this process obtains its own challenge
        // when none was injected, so every Pod of a Deployment enrolls --
        // not just the first one of a revision. With neither set, this
        // identity is assumed to have been enrolled some other way and
        // enrollment is skipped, exactly as before.
        if let (Ok(endpoint), Ok(pod_uid)) = (env::var(SERVICE_ENDPOINT_ENV), env::var(POD_UID_ENV))
        {
            let token_path = env::var(WORKLOAD_TOKEN_PATH_ENV)
                .unwrap_or_else(|_| DEFAULT_WORKLOAD_TOKEN_PATH.to_owned());
            let workload_token = std::fs::read_to_string(&token_path)
                .map_err(RulesError::EnrollmentTokenUnreadable)?
                .trim()
                .to_owned();
            let challenge = match env::var(ENROLLMENT_CHALLENGE_ENV) {
                Ok(injected) => decode_challenge(&injected)?,
                Err(_) => client.request_challenge(&pod_uid, &workload_token)?,
            };
            let enrolled = client.enroll(challenge, &endpoint, &pod_uid, workload_token)?;
            println!("enrolled with the kernel: {enrolled:?}");
            instance_lease = Some(InstanceLease {
                revision: enrolled.lease_revision,
                expires_at: enrolled.lease_expires_at,
            });
        }
        for action in dispatch::ACTIONS {
            client.ensure_subscription(action)?;
            println!("subscription active for {action}");
        }
        let database = Database::connect_from_env()?;
        let repository = PostgresRulesRepository::new(database);
        Ok(Self {
            client,
            repository,
            lease_seconds,
            poll_interval: Duration::from_secs(poll_interval_seconds),
            instance_lease,
        })
    }
}

fn decode_challenge(value: &str) -> Result<[u8; infernal_client::CHALLENGE_LENGTH], RulesError> {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| RulesError::InvalidEnrollmentChallenge)?
        .try_into()
        .map_err(|_| RulesError::InvalidEnrollmentChallenge)
}

#[derive(Debug)]
pub enum WorkOutcome {
    NothingEligible,
    ClaimLost {
        route_id: String,
    },
    RequestUnavailable {
        route_id: String,
    },
    LostBeforeCompletion {
        route_id: String,
        claim_id: String,
    },
    Completed {
        route_id: String,
        outcome: dispatch::DispatchOutcome,
    },
    UnknownAction {
        route_id: String,
        action: String,
    },
}

pub fn work_once(
    port: &impl KernelPort,
    repository: &impl AdmissionRepository,
    lease_seconds: i64,
) -> Result<WorkOutcome, RulesError> {
    let routes = port.eligible_routes()?;
    let Some(route) = routes.into_iter().next() else {
        return Ok(WorkOutcome::NothingEligible);
    };

    let claim = match port.propose_claim(&route.route_id, lease_seconds)? {
        ClaimOutcome::Claimed(claim) => claim,
        ClaimOutcome::AlreadyClaimed | ClaimOutcome::RouteNotFound => {
            return Ok(WorkOutcome::ClaimLost {
                route_id: route.route_id,
            });
        }
    };

    let request = match port.routed_request(&route.route_id)? {
        RoutedRequestOutcome::Found(request) => request,
        RoutedRequestOutcome::NotFound => {
            return Ok(WorkOutcome::RequestUnavailable {
                route_id: route.route_id,
            });
        }
    };

    // Admission and its local commit happen *before* completing the
    // kernel claim, and the claim is only completed if admission
    // succeeds (or is recognized as already done) -- see README, "domain
    // commit succeeded, but kernel completion was not recorded".
    let dispatch_result = dispatch::dispatch(&request.action, &request.scope, repository);
    let outcome = match dispatch_result {
        Ok(outcome) => outcome,
        Err(RulesError::UnknownAction(action)) => {
            port.complete_claim(&claim.claim_id, claim.fencing_token)?;
            return Ok(WorkOutcome::UnknownAction {
                route_id: route.route_id,
                action,
            });
        }
        Err(error) => return Err(error),
    };

    match port.complete_claim(&claim.claim_id, claim.fencing_token)? {
        crate::claims::CompleteOutcome::Completed(_) => Ok(WorkOutcome::Completed {
            route_id: route.route_id,
            outcome,
        }),
        crate::claims::CompleteOutcome::Fenced | crate::claims::CompleteOutcome::NotFound => {
            Ok(WorkOutcome::LostBeforeCompletion {
                route_id: route.route_id,
                claim_id: claim.claim_id,
            })
        }
    }
}

pub fn run(config: Config) -> ! {
    let Config {
        client,
        repository,
        lease_seconds,
        poll_interval,
        mut instance_lease,
    } = config;
    loop {
        renew_lease_if_due(&client, &mut instance_lease);
        match work_once(&client, &repository, lease_seconds) {
            Ok(WorkOutcome::NothingEligible) => {}
            Ok(outcome) => println!("{outcome:?}"),
            Err(error) => eprintln!("work pass failed: {error}"),
        }
        std::thread::sleep(poll_interval);
    }
}

fn renew_lease_if_due(client: &KernelClient, instance_lease: &mut Option<InstanceLease>) {
    let Some(lease) = instance_lease else {
        return;
    };
    if unix_time() < lease.expires_at - RENEWAL_MARGIN_SECONDS {
        return;
    }
    match client.renew_lease(lease.revision) {
        Ok(renewed) => {
            lease.revision = renewed.lease_revision;
            lease.expires_at = renewed.lease_expires_at;
            println!("renewed instance lease: {renewed:?}");
        }
        Err(error) => eprintln!("instance lease renewal failed: {error}"),
    }
}

fn unix_time() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(i64::MAX)
}
