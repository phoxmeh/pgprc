//! Manual test tool: connect to a live AGWPE engine (e.g. Direwolf's `-X`
//! AGWPE mode) and transmit exactly one unconnected (UI/beacon) frame, then
//! exit. Useful for verifying on-air framing against a packet capture when a
//! real two-way QSO partner isn't available.
//!
//! Usage: send_unproto <host> <tcp_port> <radio_port> <my_call> <dest_call> [message...]

use std::time::{Duration, Instant};

use pr_agwpe::client::AgwpeRunner;
use pr_core::{spawn_port, PortCommand, PortEvent};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 5 {
        eprintln!("usage: send_unproto <host> <tcp_port> <radio_port> <my_call> <dest_call> [message...]");
        eprintln!("example: send_unproto 127.0.0.1 8000 0 N0CALL-1 BEACON hello via packet radio rust client");
        std::process::exit(1);
    }
    let host = args[0].clone();
    let tcp_port: u16 = args[1].parse().expect("tcp_port must be a number");
    let radio_port: u8 = args[2].parse().expect("radio_port must be a number");
    let my_call = args[3].clone();
    let dest_call = args[4].clone();
    let message = if args.len() > 5 {
        args[5..].join(" ")
    } else {
        "Hello from the Rust packet radio client".to_string()
    };

    println!("connecting to AGWPE at {host}:{tcp_port} (radio port {radio_port})\u{2026}");
    let handle = spawn_port(AgwpeRunner {
        host,
        tcp_port,
        radio_port,
        my_call: my_call.clone(),
        login: None,
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut connected = false;
    while Instant::now() < deadline {
        match handle.events.recv_blocking() {
            Ok(PortEvent::PortConnected) => {
                connected = true;
                break;
            }
            Ok(PortEvent::PortError { message }) => {
                eprintln!("port error: {message}");
                std::process::exit(1);
            }
            Ok(other) => println!("(pre-connect event) {other:?}"),
            Err(_) => {
                eprintln!("port thread exited before connecting");
                std::process::exit(1);
            }
        }
    }
    if !connected {
        eprintln!("timed out waiting to connect");
        std::process::exit(1);
    }

    println!("connected. sending unproto frame {my_call} > {dest_call}: {message}");
    handle
        .cmd_tx
        .send(PortCommand::SendUnproto { dest: dest_call, via: Vec::new(), bytes: message.into_bytes() })
        .expect("send command");

    // Drain events for a bit so we see the engine's own log/monitor lines.
    let drain_until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < drain_until {
        match handle.events.try_recv() {
            Ok(event) => println!("{event:?}"),
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }

    let _ = handle.cmd_tx.send(PortCommand::Disconnect);
    let _ = handle.join.join();
    println!("done");
}
