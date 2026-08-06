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
    /// Load the file-trace BPF program and stream file events.
    pub fn run_sphere() -> anyhow::Result<()> {
        // libbpf-rs CO-RE loader: opens build/file_trace.bpf.o, attaches the
        // tracepoint, and reads the ring buffer. Full implementation lands
        // with the Linux build (this is the seam the sidecar hangs on).
        anyhow::bail!(
            "entheai-sphere: Linux eBPF loader not yet wired (step 6.1 — \
             libbpf-rs CO-RE, BTF at /sys/kernel/btf/vmlinux, coders=fleet)"
        )
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
