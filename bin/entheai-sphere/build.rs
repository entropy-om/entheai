//! build.rs — compiles the eBPF object ONLY on Linux (clang + libbpf headers).
//! On darwin it's a no-op so the workspace stays buildable on Apple Silicon.
//! The BPF C source is not valid on macOS (kernel headers absent).

fn main() {
    #[cfg(target_os = "linux")]
    {
        // The full Linux build wires clang + libbpf-sys here (step 6.1).
        // For now the BPF source ships as source; the loader is gated.
        println!("cargo:rerun-if-changed=bpf/file_trace.bpf.c");
    }
    // darwin: nothing to build — the stub compiles clean.
}
