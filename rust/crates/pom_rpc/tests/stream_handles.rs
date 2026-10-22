//! The pieces a terminal bridge needs: waiting for credit, input read on its own thread, acks and typing from a
//! thread other than the reader's, and a request whose answer nobody waits for.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use pom_rpc::mock;
use pom_rpc::{
    Config, Connection, RemoteError, Request, RequestContext, StreamEvent, StreamSender,
};
use serde_json::Value;

const WINDOW: u64 = 64 * 1024;

type Senders = Arc<Mutex<mpsc::Sender<StreamSender>>>;

fn connected() -> (
    Connection,
    Connection,
    mpsc::Receiver<StreamSender>,
    Arc<AtomicBool>,
) {
    let (handed, senders) = mpsc::channel();
    let handed: Senders = Arc::new(Mutex::new(handed));
    let cancelled = Arc::new(AtomicBool::new(false));
    let seen_cancel = cancelled.clone();
    let handler = move |request: Request, context: &RequestContext| -> Result<Value, RemoteError> {
        match request.method.as_str() {
            "attach" => {
                let channel = request.params.as_u64().unwrap_or_default() as u32;
                let sender = context
                    .connection()
                    .attach_stream(channel, 0)
                    .map_err(|error| RemoteError::new("stream", error.to_string()))?;
                handed
                    .lock()
                    .map_err(|_| RemoteError::new("lock", "poisoned"))?
                    .send(sender)
                    .map_err(|error| RemoteError::new("hand", error.to_string()))?;
                Ok(Value::Null)
            }
            "slow" => {
                let started = Instant::now();
                while started.elapsed() < Duration::from_millis(300) {
                    if context.is_cancelled() {
                        seen_cancel.store(true, Ordering::Release);
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Ok(Value::Null)
            }
            other => Err(RemoteError::new("unknown", other)),
        }
    };
    let config = Config {
        window: WINDOW,
        ..Config::default()
    };
    let (client, server) = mock::connected(config, handler).expect("connected");
    (client, server, senders, cancelled)
}

#[test]
fn credit_waits_for_an_ack_and_input_reads_on_its_own_thread() {
    let (client, _server, senders, _) = connected();
    let receiver = client.open_stream().expect("stream");
    client
        .request("attach", receiver.channel())
        .expect("attach");
    let mut sender = senders
        .recv_timeout(Duration::from_secs(5))
        .expect("sender");
    let input = sender.take_input().expect("input");
    assert!(sender.take_input().is_none(), "taken once");

    let typed = thread::spawn(move || input.recv_timeout(Duration::from_secs(5)));
    let handle = receiver.handle();
    handle
        .send_input(b"ls\n")
        .expect("type from another thread");
    let chunk = typed.join().expect("thread").expect("input");
    assert_eq!((chunk.offset, chunk.bytes.as_slice()), (0, &b"ls\n"[..]));

    let block = vec![b'x'; WINDOW as usize];
    assert_eq!(sender.try_write(&block).expect("fill"), WINDOW as usize);
    assert_eq!(
        sender
            .wait_credit(1, Duration::from_millis(50))
            .expect("credit"),
        0
    );

    let reading = thread::spawn(move || {
        let mut read = 0;
        while read < WINDOW {
            match receiver.recv_timeout(Duration::from_secs(5)) {
                Ok(StreamEvent::Data { bytes, .. }) => read += bytes.len() as u64,
                other => panic!("{other:?}"),
            }
        }
        receiver
    });
    let receiver = reading.join().expect("reader");
    let started = Instant::now();
    handle.ack(WINDOW);
    let credit = sender
        .wait_credit(WINDOW / 2, Duration::from_secs(5))
        .expect("credit");
    assert_eq!(credit, WINDOW);
    assert!(started.elapsed() < Duration::from_secs(1));
    drop(receiver);
}

#[test]
fn a_forgotten_request_runs_without_a_cancel() {
    let (client, _server, _, cancelled) = connected();
    client
        .start_request("slow", Value::Null)
        .expect("start")
        .forget();
    thread::sleep(Duration::from_millis(400));
    assert!(!cancelled.load(Ordering::Acquire));
    let dropped = client.start_request("slow", Value::Null).expect("start");
    drop(dropped);
    thread::sleep(Duration::from_millis(400));
    assert!(cancelled.load(Ordering::Acquire), "dropping still cancels");
}
