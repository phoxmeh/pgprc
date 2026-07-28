//! Manual test tool: connect to a live KISS TNC (TCP or serial, e.g.
//! Direwolf's KISS network port) and transmit exactly one unconnected
//! (UI/beacon) frame, then exit.
//!
//! Usage: send_unproto_kiss <tcp:host:port|serial:device:baud> <my_call> <dest_call> [message...]
//! Example: send_unproto_kiss tcp:127.0.0.1:8001 KD3BFP-9 BEACON hello via KISS
//! Example: send_unproto_kiss serial:/dev/ttyUSB0:9600 KD3BFP-9 BEACON hello via KISS

use std::time::{Duration, Instant};

use pr_ax25::{KissRunner, KissTransport};
use pr_core::{spawn_port, PortCommand, PortEvent};

fn parse_transport(spec: &str) -> KissTransport {
    let mut parts = spec.splitn(3, ':');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("tcp"), Some(host), Some(port)) => {
            KissTransport::Tcp { host: host.to_string(), port: port.parse().expect("port must be a number") }
        }
        (Some("serial"), Some(device), Some(baud)) => {
            KissTransport::Serial { device: device.to_string(), baud: baud.parse().expect("baud must be a number") }
        }
        _ => {
            eprintln!("transport must be 'tcp:host:port' or 'serial:device:baud', got '{spec}'");
            std::process::exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: send_unproto_kiss <tcp:host:port|serial:device:baud> <my_call> <dest_call> [message...]");
        eprintln!("example: send_unproto_kiss tcp:127.0.0.1:8001 KD3BFP-9 BEACON hello via KISS");
        std::process::exit(1);
    }
    let transport = parse_transport(&args[0]);
    let my_call = args[1].clone();
    let dest_call = args[2].clone();
    let message = if args.len() > 3 {
        args[3..].join(" ")
    } else {
        "Hello from the Rust packet radio client (KISS)".to_string()
    };

    println!("connecting to {}\u{2026}", args[0]);
    let handle = spawn_port(KissRunner { transport, my_call: my_call.clone() });

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
