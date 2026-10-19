//! entheai-sphere — the eBPF observability sphere around entheai's fan-out
//! POST-PRE-process layer.
//!
//! EXTRA-SENSORY PERCEPTION: the orchestrator's ordinary senses (coder
//! reports, git diffs) are augmented by kernel truth — which files the coders
//! actually touched, and where the processes actually dialed out. The sphere
//! attests; the Oracle adjudicates on attested facts.
//!
//! Platform honesty: eBPF is Linux-only. On darwin this binary compiles as a
//! stub (no eBPF claims) so the workspace stays buildable on Apple Silicon.
//! Runtime: run the sidecar on the fleet's Linux host (dev-cx53 etc.) and wire
//! its Attestations into the Oracle via NATS (`entheai.fanout.<session>.attest.*`).

use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("entheai-sphere: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::run_sphere()
    }
    #[cfg(not(target_os = "linux"))]
    {
        darwin::run_stub()
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::time::Duration;

    /// Load the file-trace BPF program and stream file events — the "eye
    /// outside the bubble": kernel truth about which files the fan-out coders
    /// actually touch, attested, not claimed.
    ///
    /// Pinned to `libbpf-rs` 0.27: `ObjectBuilder::name`/`open_memory` return
    /// `Result<&mut Self>` (not chainable into `.load()`), `Object` exposes
    /// only `progs()`/`maps()` iterators (no `prog(name)`/`map(name)`
    /// accessors), and `RingBufferBuilder::add` borrows `&dyn MapCore` rather
    /// than taking a `Map` by value. `prog.attach()` returns a `Link` that
    /// MUST be held for the poll loop's lifetime — `Link::drop` detaches the
    /// program (`bpf_link__destroy`), so a dropped temporary would silently
    /// stop delivering events into an otherwise-healthy ring buffer.
    pub fn run_sphere() -> anyhow::Result<()> {
        // CO-RE loader: the BPF object is compiled for this kernel at build
        // time (build.rs, clang + libbpf headers) and relocated at load time
        // against /sys/kernel/btf/vmlinux.
        let obj = include_bytes!(concat!(env!("OUT_DIR"), "/file_trace.bpf.o"));
        let mut builder = libbpf_rs::ObjectBuilder::default();
        builder.name("entheai-sphere")?;
        let open = builder.open_memory(obj)?;
        let obj = open.load()?;

        let prog = obj
            .progs()
            .find(|p| p.name() == "trace_openat")
            .ok_or_else(|| anyhow::anyhow!("trace_openat program missing"))?;
        // Held for the whole poll loop below — see the doc comment above.
        let _link = prog.attach()?;

        // The ring buffer delivers file events; each is an Attestation seed.
        let map = obj
            .maps()
            .find(|m| m.name() == "file_events")
            .ok_or_else(|| anyhow::anyhow!("file_events map missing"))?;
        let mut rb = libbpf_rs::RingBufferBuilder::new()?;
        rb.add(&map, |data| {
            // data layout mirrors struct file_event: u32 pid, comm[16], path[256]
            if data.len() >= 4 {
                let pid = u32::from_ne_bytes([data[0], data[1], data[2], data[3]]);
                eprintln!("attest: pid={pid} file-op (sphere eye)");
            }
            0
        })?;
        let ring = rb.build()?;

        eprintln!("entheai-sphere: eye open — attesting fan-out file ops (kernel truth)");
        loop {
            ring.poll(Duration::from_millis(200))?;
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod darwin {
    /// darwin stub — no eBPF claims. Documents the platform boundary.
    pub fn run_stub() -> anyhow::Result<()> {
        eprintln!(
            "entheai-sphere: darwin host — eBPF sphere is Linux-only. \
             Run this binary on the fleet Linux host (coders=fleet) to attest \
             the fan-out layer. Stub exiting cleanly."
        );
        Ok(())
    }
}
