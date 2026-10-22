//! Terminal latency and throughput through a real server and real holders: keystroke round trips, a 200 MiB
//! flood that arrives whole and in order without growing the process, typing in a second terminal during the
//! flood, and a client that stops acknowledging. Timings get twice the slack on CI runners.

#[path = "fixture/terminals.rs"]
mod terminals;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use pom_rpc::pty;
use pom_rpc::StreamEvent;
use terminals::{Reader, Remote, TIMEOUT};

const MEBIBYTE: u64 = 1024 * 1024;
/// A macOS pty hands its reader a few bytes per read, so there the holder, not the bridge, sets the pace.
const FLOOD: u64 = if cfg!(target_os = "linux") {
    200 * MEBIBYTE
} else {
    24 * MEBIBYTE
};

/// Timings mean nothing while another test floods the same machine.
fn alone() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn slack() -> u32 {
    if std::env::var_os("CI").is_some() {
        2
    } else {
        1
    }
}

fn percentile(sorted: &[Duration], percent: usize) -> Duration {
    let index = (sorted.len() * percent / 100).min(sorted.len().saturating_sub(1));
    sorted.get(index).copied().unwrap_or_default()
}

/// The peak resident size of this process so far, in bytes.
fn peak_rss() -> u64 {
    // SAFETY: getrusage fills the struct it is given.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer for RUSAGE_SELF.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return 0;
    }
    let peak = usage.ru_maxrss.max(0) as u64;
    if cfg!(target_os = "macos") {
        peak
    } else {
        peak * 1024
    }
}

/// Types one byte and waits for its echo; returns the round trip.
fn keystroke(terminal: &pty::Terminal, byte: u8) -> Duration {
    let started = Instant::now();
    terminal.control.input(&[byte]).expect("input");
    let deadline = started + TIMEOUT;
    loop {
        assert!(Instant::now() < deadline, "no echo of {byte}");
        match terminal.output.recv_timeout(Duration::from_millis(100)) {
            Ok(StreamEvent::Data { offset, bytes }) => {
                terminal.control.ack(offset + bytes.len() as u64);
                if bytes.contains(&byte) {
                    return started.elapsed();
                }
            }
            Ok(StreamEvent::Reset { .. }) | Err(pom_rpc::Error::Timeout) => {}
            Ok(StreamEvent::End { reason }) => panic!("ended: {reason}"),
            Err(error) => panic!("{error}"),
        }
    }
}

fn round_trips(terminal: &pty::Terminal, count: usize) -> Vec<Duration> {
    let mut times: Vec<Duration> = (0..count)
        .map(|index| keystroke(terminal, b'a' + (index % 26) as u8))
        .collect();
    times.sort();
    times
}

#[test]
fn keystrokes_echo_within_tens_of_milliseconds() {
    let _alone = alone();
    let remote = Remote::start(&["cat"]);
    let holder = remote.open(&remote.connection);
    let terminal = remote.attach(&remote.connection, &holder);
    let mut reader = Reader::new();
    terminal.control.input(b"warm\n").expect("input");
    assert!(reader.until(&terminal, "warm"));
    let times = round_trips(&terminal, 200);
    let (p50, p99) = (percentile(&times, 50), percentile(&times, 99));
    eprintln!("keystroke round trip over 200: p50 {p50:?} p99 {p99:?}");
    assert!(p50 < Duration::from_millis(30) * slack(), "p50 {p50:?}");
    assert!(p99 < Duration::from_millis(60) * slack(), "p99 {p99:?}");
}

/// `y\n` from offset 0: what each absolute offset must hold.
fn yes_at(offset: u64, length: usize) -> Vec<u8> {
    (offset..offset + length as u64)
        .map(|at| if at % 2 == 0 { b'y' } else { b'\n' })
        .collect()
}

struct Checked {
    first: Option<u64>,
    received: u64,
    end: u64,
    resets: Vec<u64>,
}

/// Reads a `yes` stream to `total`, checking every byte against its offset; `stall` holds back ACKs first.
fn read_yes(terminal: &pty::Terminal, total: u64, stall: Duration) -> Checked {
    let pattern = yes_at(0, pom_rpc::MAX_DATA + 2);
    let started = Instant::now();
    let mut checked = Checked {
        first: None,
        received: 0,
        end: 0,
        resets: Vec::new(),
    };
    let mut next: Option<u64> = None;
    let deadline = Instant::now() + Duration::from_secs(300);
    while checked.end < total {
        assert!(Instant::now() < deadline, "stalled at {}", checked.end);
        match terminal.output.recv_timeout(Duration::from_millis(100)) {
            Ok(StreamEvent::Data { offset, bytes }) => {
                if let Some(next) = next {
                    assert_eq!(offset, next, "a gap or repeat without a reset");
                }
                let phase = (offset % 2) as usize;
                assert!(
                    pattern.get(phase..phase + bytes.len()) == Some(&bytes[..]),
                    "wrong bytes at {offset}"
                );
                checked.first.get_or_insert(offset);
                checked.received += bytes.len() as u64;
                checked.end = offset + bytes.len() as u64;
                next = Some(checked.end);
                if started.elapsed() >= stall {
                    terminal.control.ack(checked.end);
                }
            }
            Ok(StreamEvent::Reset { offset }) => {
                checked.resets.push(offset);
                next = Some(offset);
            }
            Ok(StreamEvent::End { reason }) => panic!("ended at {}: {reason}", checked.end),
            Err(pom_rpc::Error::Timeout) => {
                if started.elapsed() >= stall {
                    terminal.control.ack(checked.end);
                }
            }
            Err(error) => panic!("{error}"),
        }
    }
    checked
}

fn yes_holder(remote: &Remote, name: &str, bytes: u64) {
    let script = format!("stty -opost -echo; read go; yes | head -c {bytes}; sleep 60");
    remote.spawn_holder(name, &["/bin/sh", "-c", &script]);
}

fn start_flood(terminal: &pty::Terminal) {
    std::thread::sleep(Duration::from_millis(200));
    terminal.control.input(b"\n").expect("start the flood");
}

#[test]
fn a_flood_arrives_whole_and_in_order_while_another_terminal_stays_quick() {
    let _alone = alone();
    let remote = Remote::start(&["cat"]);
    let echo = remote.attach(&remote.connection, &remote.open(&remote.connection));
    let mut warm = Reader::new();
    echo.control.input(b"warm\n").expect("input");
    assert!(warm.until(&echo, "warm"));
    let baseline = peak_rss();

    let flooding = Arc::new(AtomicBool::new(true));
    let typing = {
        let flooding = flooding.clone();
        std::thread::spawn(move || {
            let mut times = Vec::new();
            while flooding.load(Ordering::Acquire) && times.len() < 400 {
                times.push(keystroke(&echo, b'k'));
                std::thread::sleep(Duration::from_millis(10));
            }
            times.sort();
            times
        })
    };
    yes_holder(&remote, "term-demo-feat-yes-1", FLOOD);
    let started = Instant::now();
    let flood = remote.attach(&remote.connection, "term-demo-feat-yes-1");
    start_flood(&flood);
    let checked = read_yes(&flood, FLOOD, Duration::ZERO);
    let elapsed = started.elapsed();
    flooding.store(false, Ordering::Release);
    let times = typing.join().expect("typing");
    let grown = peak_rss().saturating_sub(baseline);
    let rate = FLOOD as f64 / MEBIBYTE as f64 / elapsed.as_secs_f64();
    eprintln!(
        "flood: {} MiB in {elapsed:?} ({rate:.0} MiB/s), resets {:?}, peak RSS grew {} KiB; keystrokes during it: {} p50 {:?} p99 {:?} max {:?}",
        FLOOD / MEBIBYTE,
        checked.resets,
        grown / 1024,
        times.len(),
        percentile(&times, 50),
        percentile(&times, 99),
        times.last().copied().unwrap_or_default()
    );
    assert_eq!(checked.first, Some(0), "from the first byte");
    assert_eq!(checked.received, FLOOD, "every byte, once");
    assert!(checked.resets.is_empty(), "a prompt reader never skips");
    assert!(grown < 32 * MEBIBYTE, "grew {} KiB", grown / 1024);
    assert!(!times.is_empty());
    assert!(
        percentile(&times, 99) < Duration::from_millis(50) * slack(),
        "keystrokes during the flood: {times:?}"
    );
}

#[test]
fn a_client_that_stops_acknowledging_is_reset_and_replayed() {
    let _alone = alone();
    let remote = Remote::start(&["cat"]);
    let total = 16 * MEBIBYTE;
    yes_holder(&remote, "term-demo-feat-yes-2", total);
    let flood = remote.attach(&remote.connection, "term-demo-feat-yes-2");
    start_flood(&flood);
    let checked = read_yes(&flood, total, Duration::from_secs(3));
    eprintln!(
        "stalled 3s: received {} of {} MiB, resets at {:?}",
        checked.received / MEBIBYTE,
        total / MEBIBYTE,
        checked.resets
    );
    assert!(!checked.resets.is_empty(), "the stall outran the ring");
    assert!(checked.received < total);
    assert_eq!(checked.end, total, "the stream reaches the holder's end");
}
