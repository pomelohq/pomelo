//! The live-port lookup against real processes (its own test binary, so no other test's sockets show up).

use std::net::TcpListener;

use pom_paths::StateDir;
use pom_proxy::{Machine, SystemMachine};
use pom_ptyhost::SocketDir;

#[test]
fn the_live_port_is_found_wherever_the_dev_server_listens() {
    let temp = tempfile::tempdir().expect("temp");
    let holders = SocketDir::new(temp.path().join("holders"));
    std::fs::create_dir_all(temp.path().join("holders")).expect("holders");
    let holder = "svc-demo-feat-login-api-web";
    std::fs::write(
        holders.pidfile(holder),
        format!("{}\n{holder}\n", std::process::id()),
    )
    .expect("pidfile");
    let machine = SystemMachine {
        state: StateDir::new(temp.path().join("state")),
        holders,
    };

    // Vite's 5173, CRA's 3000, Rails' 3000: the usual defaults of a server that ignores $PORT.
    let listener = (5000..9999)
        .find_map(|port| TcpListener::bind(("127.0.0.1", port)).ok())
        .expect("a free port below 10000");
    let port = listener.local_addr().expect("address").port();
    assert_eq!(machine.live_ports(holder), vec![port], "a port below 10000");
    drop(listener);

    let listener = TcpListener::bind("[::1]:0").expect("bind ::1");
    let port = listener.local_addr().expect("address").port();
    assert_eq!(machine.live_ports(holder), vec![port], "an IPv6-only port");
}
