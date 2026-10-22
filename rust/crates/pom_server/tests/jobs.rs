//! Jobs through a real server: an op id starts one job however often it is asked, a follower that went away
//! follows again from the event it last saw, sleep and wake report their stages, and bad arguments are refused.

#[path = "fixture/terminals.rs"]
mod terminals;

use std::time::{Duration, Instant};

use pom_remote::server_api::{
    job_op, method, progress_kind, BranchArgs, JobFollowParams, JobFollowed, JobStartParams,
    JobStarted, Progress, PROGRESS_TOPIC,
};
use pom_rpc::{error_code, Connection, Error, PushEvent};
use serde_json::json;
use terminals::{Remote, TIMEOUT};

fn start(
    remote: &Remote,
    connection: &Connection,
    op: &str,
    branch: &str,
    op_id: &str,
) -> JobStarted {
    let params = JobStartParams {
        cfg: remote.cfg(),
        op: op.into(),
        args: serde_json::to_value(BranchArgs {
            branch: branch.into(),
            force: false,
        })
        .expect("args"),
        op_id: op_id.into(),
    };
    serde_json::from_value(
        connection
            .request(method::JOB_START, params)
            .expect("job.start"),
    )
    .expect("started")
}

/// Follows job `id` from `since` until its last event, or `stop_after` events.
fn follow(
    connection: &Connection,
    id: &str,
    since: u64,
    stop_after: usize,
) -> (JobFollowed, Vec<Progress>) {
    let subscription = connection.subscribe().expect("subscribe");
    let params = JobFollowParams {
        id: id.into(),
        since_seq: since,
        channel: subscription.channel(),
    };
    let followed: JobFollowed = serde_json::from_value(
        connection
            .request(method::JOB_FOLLOW, params)
            .expect("job.follow"),
    )
    .expect("followed");
    let mut events = Vec::new();
    let deadline = Instant::now() + TIMEOUT;
    while events.len() < stop_after {
        assert!(Instant::now() < deadline, "only {events:?}");
        match subscription.recv_timeout(Duration::from_millis(100)) {
            Ok(PushEvent::Value { topic, value }) => {
                assert_eq!(topic, PROGRESS_TOPIC);
                let progress: Progress = serde_json::from_value(value).expect("progress");
                let done = progress.done;
                events.push(progress);
                if done {
                    break;
                }
            }
            Ok(PushEvent::End { .. }) => break,
            Err(Error::Timeout) => {}
            Err(error) => panic!("{error}"),
        }
    }
    (followed, events)
}

fn kinds(events: &[Progress]) -> Vec<&str> {
    events.iter().map(|event| event.kind.as_str()).collect()
}

#[test]
fn sleep_and_wake_report_their_stages_and_end_with_their_exit() {
    let remote = Remote::start(&["/bin/sh"]);
    let slept = start(
        &remote,
        &remote.connection,
        job_op::WS_SLEEP,
        "feat",
        "op-sleep-1",
    );
    assert!(!slept.existing);
    let (_, events) = follow(&remote.connection, &slept.id, 0, usize::MAX);
    assert_eq!(
        kinds(&events),
        [
            progress_kind::STAGES,
            progress_kind::STAGE_STARTED,
            progress_kind::STAGE_DONE,
            progress_kind::DONE
        ]
    );
    assert_eq!(events[0].stages, ["Stopping its services and agents"]);
    assert_eq!(events.last().and_then(|event| event.exit), Some(0));
    let seqs: Vec<u64> = events.iter().map(|event| event.seq).collect();
    assert_eq!(seqs, [0, 1, 2, 3]);
    let state = pom_layout::WorkspaceState::load(&remote.worktree());
    assert_eq!(state.state.name(), "sleeping");

    let woke = start(
        &remote,
        &remote.connection,
        job_op::WS_WAKE,
        "feat",
        "op-wake-1",
    );
    let (_, events) = follow(&remote.connection, &woke.id, 0, usize::MAX);
    assert_eq!(
        events.first().map(|event| event.stages.clone()),
        Some(vec!["Starting its services and agents".to_string()])
    );
    assert_eq!(events.last().and_then(|event| event.exit), Some(0));
    let state = pom_layout::WorkspaceState::load(&remote.worktree());
    assert_eq!(state.state.name(), "running");
}

#[test]
fn an_op_id_starts_one_job_and_a_follower_that_left_follows_again_from_where_it_was() {
    let remote = Remote::start(&["/bin/sh"]);
    let first = start(
        &remote,
        &remote.connection,
        job_op::WS_SLEEP,
        "feat",
        "op-once",
    );
    let other = remote.connect();
    let again = start(&remote, &other, job_op::WS_SLEEP, "feat", "op-once");
    assert_eq!(again.id, first.id);
    assert!(again.existing, "the same op id gets the same job");

    let (_, head) = follow(&other, &first.id, 0, 2);
    assert_eq!(head.len(), 2);
    other.close();
    let since = head.last().map_or(0, |event| event.seq + 1);
    let (followed, rest) = follow(&remote.connection, &first.id, since, usize::MAX);
    assert_eq!(followed.op, job_op::WS_SLEEP);
    assert_eq!(
        rest.first().map(|event| event.seq),
        Some(since),
        "it picks up at since_seq"
    );
    let all: Vec<Progress> = head.into_iter().chain(rest).collect();
    let seqs: Vec<u64> = all.iter().map(|event| event.seq).collect();
    assert_eq!(
        seqs,
        (0..all.len() as u64).collect::<Vec<_>>(),
        "nothing lost or repeated"
    );
    assert!(all.last().is_some_and(|event| event.done));

    let deadline = Instant::now() + TIMEOUT;
    while remote.handle.info().jobs != 0 {
        assert!(Instant::now() < deadline, "the job never counted as ended");
        std::thread::sleep(Duration::from_millis(20));
    }
    remote.handle.stop();
}

#[test]
fn bad_jobs_and_arguments_are_refused() {
    let remote = Remote::start(&["/bin/sh"]);
    let refused = |params: serde_json::Value, code: &str| match remote
        .connection
        .request(method::JOB_START, &params)
    {
        Err(Error::Remote(error)) => assert_eq!(error.code, code, "{params}: {}", error.message),
        other => panic!("{params}: {other:?}"),
    };
    let cfg = remote.cfg();
    refused(
        json!({"cfg": cfg, "op": "rm", "args": {}, "op_id": "a"}),
        error_code::INVALID_PARAMS,
    );
    refused(
        json!({"cfg": cfg, "op": "ws.wake", "args": {"branch": "feat"}, "op_id": "a b"}),
        error_code::INVALID_PARAMS,
    );
    refused(
        json!({"cfg": cfg, "op": "ws.wake", "args": {"branch": "../x"}, "op_id": "b"}),
        error_code::INVALID_PARAMS,
    );
    refused(
        json!({"cfg": cfg, "op": "ws.wake", "args": {"branch": 7}, "op_id": "c"}),
        error_code::INVALID_PARAMS,
    );
    refused(
        json!({"cfg": cfg, "op": "ws.create", "args": {"branch": "x", "repos": ["../api"]}, "op_id": "d"}),
        error_code::INVALID_PARAMS,
    );
    refused(
        json!({"cfg": cfg, "op": "ws.create", "args": {"branch": "x", "from_stage": 99}, "op_id": "e"}),
        error_code::INVALID_PARAMS,
    );
    refused(
        json!({"cfg": cfg, "op": "start_wait", "args": {"branch": "feat", "target": "web", "timeout_seconds": 999999}, "op_id": "f"}),
        error_code::INVALID_PARAMS,
    );
    refused(
        json!({"cfg": "/etc/passwd", "op": "ws.wake", "args": {"branch": "feat"}, "op_id": "g"}),
        error_code::INVALID_PARAMS,
    );
    refused(
        json!({"cfg": cfg, "op": "ws.wake", "args": {"branch": "nope"}, "op_id": "h"}),
        pom_server::FAILED,
    );
    match remote
        .connection
        .request(method::JOB_CANCEL, json!({"id": "job-404"}))
    {
        Err(Error::Remote(error)) => assert_eq!(error.code, pom_server::FAILED),
        other => panic!("{other:?}"),
    }
    assert_eq!(remote.handle.info().jobs, 0, "nothing started");
}
