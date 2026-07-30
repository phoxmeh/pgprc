//! Manual test tool: open a Linux kernel `AF_AX25` raw socket on a
//! `kissattach`'d device (an entry in `/etc/ax25/axports`) and transmit
//! exactly one unconnected (UI/beacon) frame, then exit.
//!
//! Usage: send_unproto_raw <axports_device> <dest_call> [message...]
//! Example: send_unproto_raw wl2k BEACON hello via the kernel AX.25 stack

use std::time::{Duration, Instant};

use pr_ax25::Ax25RawSocketRunner;
use pr_core::{spawn_port, PortCommand, PortEvent};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: send_unproto_raw <axports_device> <dest_call> [message...]");
        eprintln!("example: send_unproto_raw wl2k BEACON hello via the kernel AX.25 stack");
        std::process::exit(1);
    }
    let device = args[0].clone();
    let dest_call = args[1].clone();
    let message = if args.len() > 2 {
        args[2..].join(" ")
    } else {
        "Hello from the Rust packet radio client (raw AX.25)".to_string()
    };

    println!("binding to axports device '{device}'\u{2026}");
    let handle = spawn_port(Ax25RawSocketRunner { device });

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

    println!("bound. sending unproto frame > {dest_call}: {message}");
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
