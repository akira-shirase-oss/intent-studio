//! The hostile child the browser-confinement proofs run (BROWSER-CONFINEMENT):
//! one attack per argv word, each printing a `GOT-*` marker when the wall its
//! class names is absent. Built only under `mutation-seams` — a shipped tree
//! carries no probe.

use std::io::Write;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        // A TCP connect that can only succeed with a route: inside a fresh
        // network namespace there is none, not even to a public resolver.
        Some("net") => {
            // 127.0.0.1:59432 is the stack's Postgres — in a fresh network
            // namespace even loopback is down; on the host it answers at once.
            if let Ok(addr) = "127.0.0.1:59432".parse()
                && std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_ok()
            {
                println!("GOT-NET");
            }
        }
        // A host path outside the bundle — readable only when the job shares
        // the host's mount namespace.
        Some("read") => {
            if let Ok(bytes) = std::fs::read("/etc/passwd")
                && !bytes.is_empty()
            {
                println!("GOT-READ:{}", bytes.len());
            }
        }
        // Fork-bomb shape via respawns of itself: without an NPROC limit the
        // count climbs far past the confined bound before the group is killed.
        Some("fork") => {
            let mut spawned = 0_u32;
            for _ in 0..512 {
                match std::process::Command::new(&args[0]).arg("hang").spawn() {
                    Ok(_) => spawned += 1,
                    Err(_) => break,
                }
            }
            // Exits at once — the spawned children die with the namespace when
            // the monitor that owns pid 1 exits.
            println!("GOT-FORK:{spawned}");
        }
        // Grow the heap to a caller-bounded target (MiB): the confined run dies
        // long before it; an unbounded run reaches it and reports.
        Some("mem") => {
            let target = args
                .get(2)
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(1024)
                * 1024
                * 1024;
            let mut heap: Vec<u8> = Vec::new();
            while heap.len() < target {
                heap.resize(heap.len() + 64 * 1024 * 1024, 1);
                std::hint::black_box(&heap);
            }
            println!("GOT-MEM:{}", heap.len());
        }
        // A page that never finishes loading.
        Some("hang") => std::thread::sleep(Duration::from_secs(600)),
        // A report that never stops writing.
        Some("big") => {
            let mut out = std::io::stdout().lock();
            let block = vec![b'x'; 65_536];
            while out.write_all(&block).is_ok() {}
        }
        _ => println!("PROBE-OK"),
    }
}
