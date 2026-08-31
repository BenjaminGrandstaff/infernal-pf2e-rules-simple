//! Goal: give a human a terminal interface to the review queue
//! (`AdmissionRepository::hold_candidate`'s pending entries) -- list
//! what's waiting, inspect one in full, and resolve it to either an
//! admitted rule or a permanent discard.
//!
//! Connects straight to `PF2E_RULES_DATABASE_URL`, the same trust
//! boundary as already having database or cluster access -- no network
//! listener, no new auth surface to design or get wrong. Run it with
//! `kubectl exec` into the pod, or locally against a port-forwarded
//! database.

use std::env;
use std::process::ExitCode;

use infernal_pf2e_rules_simple::database::Database;
use infernal_pf2e_rules_simple::domain::{
    AdmissionRepository, HeldCandidate, HoldResolution, ResolutionOutcome,
};
use infernal_pf2e_rules_simple::postgres_rules_repository::PostgresRulesRepository;
use uuid::Uuid;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let command = args.next();

    let repository = match connect() {
        Ok(repository) => repository,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    let result = match command.as_deref() {
        Some("list") => list(&repository),
        Some("show") => held_id_arg(&mut args, "show").and_then(|id| show(&repository, id)),
        Some("admit") => held_id_arg(&mut args, "admit")
            .and_then(|id| resolve(&repository, id, HoldResolution::Admit)),
        Some("discard") => held_id_arg(&mut args, "discard")
            .and_then(|id| resolve(&repository, id, HoldResolution::Discard)),
        _ => Err(usage()),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn usage() -> String {
    "usage: review <list | show <held_id> | admit <held_id> | discard <held_id>>".to_owned()
}

fn held_id_arg(args: &mut impl Iterator<Item = String>, command: &str) -> Result<Uuid, String> {
    let value = args
        .next()
        .ok_or_else(|| format!("usage: review {command} <held_id>"))?;
    value
        .parse()
        .map_err(|_| format!("{value:?} is not a valid held_id (must be a UUID)"))
}

fn connect() -> Result<PostgresRulesRepository, String> {
    let database = Database::connect_from_env().map_err(|error| error.to_string())?;
    Ok(PostgresRulesRepository::new(database))
}

fn list(repository: &PostgresRulesRepository) -> Result<(), String> {
    let pending = repository
        .list_pending_held()
        .map_err(|error| error.to_string())?;
    if pending.is_empty() {
        println!("no candidates pending review");
        return Ok(());
    }
    for held in &pending {
        print_summary(held);
    }
    Ok(())
}

fn show(repository: &PostgresRulesRepository, held_id: Uuid) -> Result<(), String> {
    let held = repository
        .get_held(held_id)
        .map_err(|error| error.to_string())?;
    print_detail(&held);
    Ok(())
}

fn resolve(
    repository: &PostgresRulesRepository,
    held_id: Uuid,
    resolution: HoldResolution,
) -> Result<(), String> {
    let outcome = repository
        .resolve_held(held_id, resolution)
        .map_err(|error| error.to_string())?;
    match outcome {
        ResolutionOutcome::Admitted(admission) => {
            let already = if admission.was_already_processed {
                " (already resolved)"
            } else {
                ""
            };
            println!(
                "admitted: rule_id={} version={}{already}",
                admission.rule_id, admission.version
            );
        }
        ResolutionOutcome::Discarded => println!("discarded"),
    }
    Ok(())
}

fn print_summary(held: &HeldCandidate) {
    println!(
        "{}  {:<12} {:<30} {}",
        held.held_id,
        held.candidate.rule_type.as_str(),
        held.candidate.name.as_deref().unwrap_or("(unnamed)"),
        held.reason,
    );
}

fn print_detail(held: &HeldCandidate) {
    println!("held_id:         {}", held.held_id);
    println!("candidate_id:    {}", held.candidate.candidate_id);
    println!("rule_type:       {}", held.candidate.rule_type.as_str());
    println!(
        "name:            {}",
        held.candidate.name.as_deref().unwrap_or("(unnamed)")
    );
    println!("confidence:      {:.2}", held.candidate.confidence);
    println!("parser_version:  {}", held.candidate.parser_version);
    println!("reason:          {}", held.reason);
    println!("document_id:     {}", held.candidate.source.document_id);
    println!(
        "document_version:{}",
        held.candidate.source.document_version
    );
    println!("location:        {}", held.candidate.source.location);
}
