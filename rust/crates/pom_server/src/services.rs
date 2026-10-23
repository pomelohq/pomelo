//! What the Services panel of a remote app asks: the workspace's facts, one service started, stopped,
//! restarted or moved to a new port, the shared services, a mode or env profile picked, a shared service's
//! output. Each answers what the panel does here on its own machine.

use pom_remote::server_api::{
    ServiceChoiceParams, ServiceRunParams, SharedLogsParams, SharedRunParams, WorkspaceParams,
};
use pom_remote::{LocalSource, ServiceVerb, SourceError, WORKSPACE_REPO};
use pom_rpc::{error_code, RemoteError};
use serde_json::Value;

use crate::dispatch::{check_branch, check_name, check_op_id, Ops, FAILED};

const MAX_SHARED_LINES: usize = 5_000;

fn invalid(message: impl Into<String>) -> RemoteError {
    RemoteError::new(error_code::INVALID_PARAMS, message)
}

fn failed(error: SourceError) -> RemoteError {
    RemoteError::new(FAILED, error.to_string())
}

fn to_value(value: impl serde::Serialize) -> Result<Value, RemoteError> {
    serde_json::to_value(value)
        .map_err(|error| RemoteError::new(error_code::INTERNAL, error.to_string()))
}

fn verb(action: &str) -> Result<ServiceVerb, RemoteError> {
    ServiceVerb::parse(action)
        .ok_or_else(|| invalid("action must be start, stop, restart or relocate"))
}

/// `repo` is a repo's name or `_ws`; both it and `service` are plain names.
fn check_service(repo: &str, service: &str) -> Result<(), RemoteError> {
    if repo != WORKSPACE_REPO {
        check_name("repo", repo)?;
    }
    check_name("service", service)?;
    if repo.contains('/') || service.contains('/') {
        return Err(invalid("repo and service are single names"));
    }
    Ok(())
}

pub fn facts(source: &LocalSource, params: &WorkspaceParams) -> Result<Value, RemoteError> {
    check_branch(&params.branch)?;
    to_value(source.services_facts(&params.branch).map_err(failed)?)
}

pub fn run(
    source: &LocalSource,
    ops: &Ops,
    params: ServiceRunParams,
) -> Result<Value, RemoteError> {
    check_branch(&params.branch)?;
    check_service(&params.repo, &params.service)?;
    check_op_id(&params.op_id)?;
    let verb = verb(&params.action)?;
    let result = ops.run(&params.op_id, || {
        source
            .run_one(&params.branch, &params.repo, &params.service, verb)
            .map(|()| String::new())
            .map_err(|error| error.to_string())
    });
    result
        .map(|_| Value::Null)
        .map_err(|message| RemoteError::new(FAILED, message))
}

pub fn run_shared(
    source: &LocalSource,
    ops: &Ops,
    params: SharedRunParams,
) -> Result<Value, RemoteError> {
    if params.name != "*" {
        check_name("shared service", &params.name)?;
    }
    check_op_id(&params.op_id)?;
    let verb = verb(&params.action)?;
    if verb == ServiceVerb::Relocate {
        return Err(invalid("a shared service cannot be relocated"));
    }
    let result = ops.run(&params.op_id, || {
        source
            .run_shared_service(&params.name, verb)
            .map(|()| String::new())
            .map_err(|error| error.to_string())
    });
    result
        .map(|_| Value::Null)
        .map_err(|message| RemoteError::new(FAILED, message))
}

pub fn mode(source: &LocalSource, params: ServiceChoiceParams) -> Result<Value, RemoteError> {
    check_branch(&params.branch)?;
    check_service(&params.repo, &params.service)?;
    check_name("mode", &params.value)?;
    source
        .pick_mode(&params.branch, &params.repo, &params.service, &params.value)
        .map_err(failed)?;
    Ok(Value::Null)
}

pub fn env_profile(
    source: &LocalSource,
    params: ServiceChoiceParams,
) -> Result<Value, RemoteError> {
    check_branch(&params.branch)?;
    check_service(&params.repo, &params.service)?;
    check_name("env profile", &params.value)?;
    source
        .pick_env_profile(&params.branch, &params.repo, &params.service, &params.value)
        .map_err(failed)?;
    Ok(Value::Null)
}

pub fn shared_logs(source: &LocalSource, params: SharedLogsParams) -> Result<Value, RemoteError> {
    check_name("shared service", &params.name)?;
    if params.lines > MAX_SHARED_LINES {
        return Err(invalid(format!("lines must be at most {MAX_SHARED_LINES}")));
    }
    to_value(
        source
            .shared_output(&params.name, params.lines)
            .map_err(failed)?,
    )
}
